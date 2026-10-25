use crate::manager::Result;
use reqwest::{blocking::Client, redirect::Policy, Url};
use std::{
    fs::File,
    path::{Path, PathBuf},
};
use std::{
    io::{self, Read},
    time::Duration,
};
use tempfile::{Builder, TempDir};

pub struct Download {
    _directory: TempDir,
    path: PathBuf,
}
impl Download {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

const MAX_DOWNLOAD: u64 = 2 * 1024 * 1024 * 1024;

pub fn resolve(source: &str) -> Result<Option<Download>> {
    if !source.contains("://") {
        return Ok(None);
    }
    let url = Url::parse(source)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(
            io::Error::other("Use an HTTP(S) file URL without embedded credentials").into(),
        );
    }
    let filename = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or("");
    if ![".dpk", ".dpkx", ".tar.gz", ".deb", ".rpm"]
        .iter()
        .any(|suffix| filename.ends_with(suffix))
        || filename.contains(['\\', ':'])
    {
        return Err(io::Error::other("URL path must end in .dpk, .dpkx, .tar.gz, .deb or .rpm").into());
    }
    let filename = filename.to_owned();
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(600))
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                attempt.error("Too many redirects")
            } else if !matches!(attempt.url().scheme(), "http" | "https")
                || (attempt.url().scheme() == "http"
                    && attempt.previous().iter().any(|u| u.scheme() == "https"))
            {
                attempt.error("Unsafe redirect")
            } else {
                attempt.follow()
            }
        }))
        .user_agent(concat!("dev/", env!("CARGO_PKG_VERSION")))
        .build()?;
    crate::progress::stage("Downloading package...");
    // Avoid printing signed URLs or reqwest errors containing their query tokens.
    let response = client
        .get(url)
        .send()
        .map_err(|err| err.without_url())?
        .error_for_status()
        .map_err(|err| err.without_url())?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_DOWNLOAD)
    {
        return Err(io::Error::other("Download exceeds 2 GiB limit").into());
    }
    let directory = Builder::new().prefix("dev-download-").tempdir()?;
    let path = directory.path().join(filename);
    let mut file = File::create(&path)?;
    let size = io::copy(&mut response.take(MAX_DOWNLOAD + 1), &mut file)?;
    if size > MAX_DOWNLOAD {
        return Err(io::Error::other("Download exceeds 2 GiB limit").into());
    }
    if size == 0 {
        return Err(io::Error::other("Downloaded package is empty").into());
    }
    Ok(Some(Download {
        _directory: directory,
        path,
    }))
}
