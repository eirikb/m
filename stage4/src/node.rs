use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::pin::Pin;
use log::info;

use scraper::{Html, Selector};
use semver::VersionReq;
use serde::Deserialize;
use serde::Serialize;
use package_json::PackageJsonManager;
use regex::Regex;

use crate::executor::{AppInput, Download, Executor};
use crate::target::{Arch, Os, Target, Variant};
use crate::version::GGVersion;

type Root = Vec<Root2>;

#[derive(Serialize, Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
enum LTS {
    String(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Root2 {
    pub version: String,
    pub date: String,
    pub files: Vec<String>,
    pub npm: String,
    pub v8: String,
    pub uv: String,
    pub zlib: String,
    pub openssl: String,
    pub modules: String,
    pub lts: LTS,
    pub security: bool,
}

pub struct Node {
    pub cmd: String,
}

fn get_package_version() -> Option<Box<VersionReq>> {
    let mut manager = PackageJsonManager::new();
    if manager.locate_closest().is_ok() {
        if let Ok(json) = manager.read_ref() {
            if json.engines.is_some() {
                return Some(Box::new(VersionReq::parse(json.clone().engines.clone().unwrap().get("node").unwrap_or(&"".to_string())).unwrap_or(VersionReq::default())));
            }
        }
    }


    if let Ok(nvmrc) = fs::read_to_string(".nvmrc") {
        let nvmrc = Regex::new("^v").unwrap().replace(&nvmrc, "");
        let nvmrc = nvmrc.trim();
        info!("Got version {nvmrc} from .nvmrc");
        if let Ok(ver) = VersionReq::parse(&nvmrc) {
            info!("Got parsed version {ver} from .nvmrc");
            return Some(Box::new(ver.clone()));
        }
    }
    None
}

impl Executor for Node {
    fn get_version_req(&self) -> Option<VersionReq> {
        if let Some(v) = get_package_version() {
            Some(*v)
        } else {
            None
        }
    }

    fn get_download_urls<'a>(&self, input: &'a AppInput) -> Pin<Box<dyn Future<Output=Vec<Download>> + 'a>> {
        Box::pin(async move { get_node_urls(&input.target).await })
    }

    fn get_bin(&self, input: &AppInput) -> &str {
        match &input.target.os {
            Os::Windows => match self.cmd.as_str() {
                "node" => "node.exe",
                "npm" => "npm.cmd",
                _ => "npx.cmd",
            },
            _ => match self.cmd.as_str() {
                "node" => "bin/node",
                "npm" => "bin/npm",
                _ => "bin/npx"
            }
        }
    }

    fn get_name(&self) -> &str {
        "node"
    }
}

async fn official_downloads(target: &Target) -> Vec<Download> {
    let file = match (target.os, target.arch) {
        (Os::Windows, _) => "win-x64.zip",
        (Os::Linux, Arch::Armv7) => "linux-armv7l.tar.gz",
        (Os::Linux, Arch::Arm64) => "linux-arm64.tar.gz",
        (Os::Linux, _) => "linux-x64.tar.gz",
        (Os::Mac, Arch::Arm64) => "darwin-arm64.tar.gz",
        (Os::Mac, _) => "darwin-x64.tar.gz",
    };
    let body = reqwest::get("https://nodejs.org/en/download/releases/").await
        .expect("Unable to connect to nodejs.org").text().await
        .expect("Unable to download nodejs list of versions");

    let document = Html::parse_document(body.as_str());
    let rows = Selector::parse("#tbVersions tbody tr").unwrap();

    document.select(&rows).filter_map(|row| {
        let fields: HashMap<String, String> = row.select(&Selector::parse("td").unwrap()).filter_map(|td| {
            let value = td.value();
            let data_label = value.attr("data-label");
            match data_label {
                Some(data_label) => Some((data_label.trim().to_string(), td.text().next().unwrap_or("").replace("Node.js", "").trim().to_string())),
                _ => None
            }
        }).collect();
        if fields.contains_key("Version") {
            let version = fields["Version"].to_string();
            let lts = fields.contains_key("LTS") && fields["LTS"].len() > 0;
            let set: HashSet<String> = if lts {
                ["lts".to_string()].iter().cloned().collect()
            } else {
                HashSet::new()
            };

            return Some(Download::new(format!("https://nodejs.org/download/release/v{version}/node-v{version}-{file}"), version.as_str()));
        }
        None
    }).collect()
}

async fn unofficial_downloads(target: &Target) -> Vec<Download> {
    let file = match (target.os, target.arch, target.variant) {
        (Os::Windows, Arch::Arm64, _) => "win-arm64-zip",
        (Os::Windows, _, _) => "win-x64-zip",
        (Os::Linux, Arch::Armv7, Some(Variant::Musl)) => "linux-armv7l-musl",
        (Os::Linux, Arch::Arm64, Some(Variant::Musl)) => "linux-arm64-musl",
        (Os::Linux, Arch::X86_64, Some(Variant::Musl)) => "linux-x64-musl",
        (Os::Linux, Arch::Armv7, _) => "linux-armv7l",
        (Os::Linux, Arch::Arm64, _) => "linux-arm64",
        _ => "linux-x64",
    };
    let json = reqwest::get("https://unofficial-builds.nodejs.org/download/release/index.json").await.unwrap().text().await.unwrap();
    let root: Root = serde_json::from_str(json.as_str()).expect("JSON was not well-formatted");

    root.iter().rev().filter(|r|
        r.files.contains(&file.to_string())
    ).map(|r| {
        let lts = match r.lts {
            LTS::String(_) => true,
            _ => false
        };
        let file_fix = if file.ends_with("-zip") {
            file.replace("-zip", ".zip")
        } else {
            file.to_string() + ".tar.gz"
        };
        let version = r.clone().version;
        let set: HashSet<String> = if lts {
            ["lts".to_string()].iter().cloned().collect()
        } else {
            HashSet::new()
        };
        return Download::new(format!("https://unofficial-builds.nodejs.org/download/release/{version}/node-{version}-{file_fix}"), version.as_str());
    }).collect()
}

async fn get_node_urls(target: &Target) -> Vec<Download> {
    match (target.os, target.arch, target.variant) {
        (Os::Linux, _, Some(Variant::Musl)) => unofficial_downloads(target).await,
        (Os::Windows, Arch::Arm64, _) => unofficial_downloads(target).await,
        _ => official_downloads(target).await
    }
}
