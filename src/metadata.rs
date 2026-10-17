use crate::manager::{Manifest, Result};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Repository {
    Url(String),
    Details(RepositoryDetails),
}
impl Default for Repository {
    fn default() -> Self {
        Self::Url(String::new())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryDetails {
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub directory: String,
    #[serde(default)]
    pub release: Option<Release>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    #[serde(rename = "type")]
    pub kind: String,
    pub filename: String,
    #[serde(default = "default_tag")]
    pub tag: String,
    #[serde(default)]
    pub url: String,
}
fn default_tag() -> String {
    "v{version}".into()
}

#[derive(Debug, Serialize)]
pub struct ResolvedRelease {
    pub version: String,
    pub tag: String,
    pub filename: String,
    pub url: String,
}

fn invalid(message: &str) -> Box<dyn std::error::Error> {
    io::Error::other(message).into()
}
fn expand(template: &str, manifest: &Manifest) -> Result<String> {
    let value = template
        .replace("{version}", &manifest.version)
        .replace("{name}", &manifest.name);
    if value.contains(['{', '}']) {
        return Err(invalid(
            "Unknown release template; use {version} and {name}",
        ));
    }
    Ok(value)
}

impl Manifest {
    pub fn resolved_release(&self) -> Result<Option<ResolvedRelease>> {
        let Repository::Details(repository) = &self.repository else {
            return Ok(None);
        };
        if repository.kind != "git" {
            return Err(invalid("repository.type must be git"));
        }
        let Some(release) = &repository.release else {
            return Ok(None);
        };
        if !self.release.is_empty() {
            return Err(invalid(
                "Use repository.release instead of also setting top-level release",
            ));
        }
        let suffix = match release.kind.as_str() {
            "dpk" => ".dpk",
            "tar.gz" => ".tar.gz",
            _ => return Err(invalid("release.type must be dpk or tar.gz")),
        };
        let tag = expand(&release.tag, self)?;
        let mut filename = expand(&release.filename, self)?;
        if !filename.ends_with(suffix) {
            filename.push_str(suffix);
        }
        if filename == suffix
            || filename.contains(['/', '\\', ':'])
            || filename.chars().any(char::is_control)
            || tag.is_empty()
            || tag.contains('/')
            || tag.chars().any(char::is_control)
        {
            return Err(invalid("Invalid release filename or tag"));
        }
        let url = if !release.url.is_empty() {
            let template = release
                .url
                .replace("{tag}", &tag)
                .replace("{filename}", &filename);
            expand(&template, self)?
        } else {
            let mut url = reqwest::Url::parse(&repository.url)?;
            if url.scheme() != "https"
                || url.host_str() != Some("github.com")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(invalid(
                    "Set repository.release.url for repositories outside https://github.com",
                ));
            }
            let path = url
                .path()
                .trim_matches('/')
                .trim_end_matches(".git")
                .to_owned();
            if path.split('/').count() != 2 || path.split('/').any(str::is_empty) {
                return Err(invalid(
                    "GitHub repository URL must contain owner/repository",
                ));
            }
            url.set_path(&path);
            url.path_segments_mut()
                .map_err(|_| invalid("Invalid repository URL"))?
                .push("releases")
                .push("download")
                .push(&tag)
                .push(&filename);
            url.to_string()
        };
        let parsed = reqwest::Url::parse(&url)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(invalid(
                "release.url must be an HTTP(S) URL without embedded credentials",
            ));
        }
        Ok(Some(ResolvedRelease {
            version: self.version.clone(),
            tag,
            filename,
            url,
        }))
    }
}
