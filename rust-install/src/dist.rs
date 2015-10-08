
use temp;
use errors::*;
use utils;

use std::path::Path;
use std::fmt;
use std::env;

use regex::Regex;
use hyper;

pub const DEFAULT_DIST_ROOT: &'static str = "https://static.rust-lang.org/dist";
pub const UPDATE_HASH_LEN: usize = 20;

pub struct ToolchainDesc {
	pub arch: Option<String>,
	pub os: Option<String>,
	pub env: Option<String>,
	pub channel: String,
	pub date: Option<String>,
}

impl ToolchainDesc {
	pub fn from_str(name: &str) -> Option<Self> {
		let archs = ["i686", "x86_64"];
		let oses = ["pc-windows", "unknown-linux", "apple-darwin"];
		let envs = ["gnu", "msvc"];
		let channels = ["nightly", "beta", "stable"];
		
		let pattern = format!(
			r"^(?:({})-)?(?:({})-)?(?:({})-)?({})(?:-(\d{{4}}-\d{{2}}-\d{{2}}))?$",
			archs.join("|"), oses.join("|"), envs.join("|"), channels.join("|")
			);
		
		let re = Regex::new(&pattern).unwrap();
		re.captures(name).map(|c| {
			fn fn_map(s: &str) -> Option<String> {
				if s == "" {
					None
				} else {
					Some(s.to_owned())
				}
			}
				
			ToolchainDesc {
				arch: c.at(1).and_then(fn_map),
				os: c.at(2).and_then(fn_map),
				env: c.at(3).and_then(fn_map),
				channel: c.at(4).unwrap().to_owned(),
				date: c.at(5).and_then(fn_map),
			}
		})
	}
	
	pub fn manifest_url(&self, dist_root: &str) -> String {
		match self.date {
			None =>
				format!("{}/channel-rust-{}", dist_root, self.channel),
			Some(ref date) =>
				format!("{}/{}/channel-rust-{}", dist_root, date, self.channel),
		}
	}
	
	pub fn package_dir(&self, dist_root: &str) -> String {
		match self.date {
			None =>
				format!("{}", dist_root),
			Some(ref date) =>
				format!("{}/{}", dist_root, date),
		}
	}
	
	pub fn target_triple(&self) -> Option<String> {
		let (host_arch, host_os, host_env) = get_host_triple();
		let arch = self.arch.as_ref().map(|s| &**s).unwrap_or(host_arch);
		let os = self.os.as_ref().map(|s| &**s).or(host_os);
		let env = self.env.as_ref().map(|s| &**s).or(host_env);
		
		os.map(|os| {
			if let Some(ref env) = env {
				format!("{}-{}-{}", arch, os, env)
			} else {
				format!("{}-{}", arch, os)
			}
		})
	}
	
	pub fn download_manifest<'a>(&self, cfg: DownloadCfg<'a>) -> Result<Manifest<'a>> {
		let url = self.manifest_url(cfg.dist_root);
		let package_dir = self.package_dir(cfg.dist_root);
		
		let manifest = try!(download_and_check(&url, None, "", cfg)).unwrap().0;
		
		Ok(Manifest(manifest, package_dir))
	}
}

pub struct Manifest<'a>(temp::File<'a>, String);

impl<'a> Manifest<'a> {
	pub fn package_url(&self, package: &str, target_triple: &str, ext: &str) -> Result<Option<String>> {
		let suffix = target_triple.to_owned() + ext;
		utils::match_file("manifest", &self.0, |line| {
			if line.starts_with(package) && line.ends_with(&suffix) {
				Some(format!("{}/{}", &self.1, line))
			} else {
				None
			}
		})
	}
}

impl fmt::Display for ToolchainDesc {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		if let Some(ref arch) = self.arch {
			try!(write!(f, "{}-", arch));
		}
		if let Some(ref os) = self.os {
			try!(write!(f, "{}-", os));
		}
		if let Some(ref env) = self.env {
			try!(write!(f, "{}-", env));
		}
		
		try!(write!(f, "{}", &self.channel));
		
		if let Some(ref date) = self.date {
			try!(write!(f, "-{}", date));
		}
		
		Ok(())
	}
}
fn parse_url(url: &str) -> Result<hyper::Url> {
	hyper::Url::parse(url).map_err(|_| Error::InvalidUrl)
}

pub fn download_and_check<'a>(url: &str, update_hash: Option<&Path>, ext: &str, cfg: DownloadCfg<'a>) -> Result<Option<(temp::File<'a>, String)>> {
	let hash = try!(download_hash(url, cfg));
	let partial_hash: String = hash.chars().take(UPDATE_HASH_LEN).collect();
	
	if let Some(hash_file) = update_hash {
		if utils::is_file(hash_file) {
			if let Ok(contents) = utils::read_file("update hash", hash_file) {
				if contents == partial_hash {
					// Skip download, update hash matches
					cfg.notify_handler.call(Notification::UpdateHashMatches(&partial_hash));
					return Ok(None);
				}
			} else {
				cfg.notify_handler.call(Notification::CantReadUpdateHash(hash_file));
			}
		} else {
			cfg.notify_handler.call(Notification::NoUpdateHash(hash_file));
		}
	}
	
	let url = try!(parse_url(url));
	let file = try!(cfg.temp_cfg.new_file_with_ext(ext));
	try!(utils::download_file(url, &file, cfg.notify_handler));
	// TODO: Actually download and check the checksum and signature of the file
	Ok(Some((file, partial_hash)))
}

#[derive(Copy, Clone)]
pub struct DownloadCfg<'a> {
	pub dist_root: &'a str,
	pub temp_cfg: &'a temp::Cfg,
	pub notify_handler: &'a NotifyHandler,
}

pub fn download_dist<'a>(toolchain: &str, update_hash: Option<&Path>, cfg: DownloadCfg<'a>) -> Result<Option<(temp::File<'a>, String)>> {
	let desc = try!(ToolchainDesc::from_str(toolchain)
		.ok_or(Error::InvalidToolchainName));
	
	let target_triple = try!(desc.target_triple().ok_or(Error::UnsupportedHost));
	let ext = get_installer_ext();
	
	let manifest = try!(desc.download_manifest(cfg));
	
	let maybe_url = try!(manifest.package_url("rust", &target_triple, ext));
	
	let url = try!(maybe_url.ok_or(Error::UnsupportedHost));
	
	download_and_check(&url, update_hash, ext, cfg)
}

pub fn get_host_triple() -> (&'static str, Option<&'static str>, Option<&'static str>) {
	let arch = env::consts::ARCH;
	
	let os = match env::consts::OS {
		"windows" => Some("pc-windows"),
		"linux" => Some("unknown-linux"),
		"macos" => Some("apple-darwin"),
		_ => None,
	};
	
	let env = match () {
		() if cfg!(target_env = "gnu") => Some("gnu"),
		() if cfg!(target_env = "msvc") => Some("msvc"),
		_ => None,
	};
	
	(arch, os, env)
}

pub fn get_installer_ext() -> &'static str {
	if cfg!(windows) {
		if env::var_os("MSYSTEM").and_then(utils::if_not_empty).is_none() {
			return ".msi"
		}
	}
	".tar.gz"
}

pub fn download_hash(url: &str, cfg: DownloadCfg) -> Result<String> {
	let hash_url = try!(parse_url(&(url.to_owned() + ".sha256")));
	let hash_file = try!(cfg.temp_cfg.new_file());
	
	try!(utils::download_file(hash_url, &hash_file, cfg.notify_handler));
	
	utils::read_file("hash", &hash_file)
}