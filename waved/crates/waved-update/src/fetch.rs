//! Getting the index and the package. `Fetcher` is the seam; `HttpFetcher`
//! is the device's, `FakeFetcher` the host fake.
//!
//! Downloads resume: `download` appends to whatever part of the file is
//! already there, asking the server for the rest with a `Range` header. A
//! server that ignores ranges sends the whole file again, which starts the
//! file over.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::{StatusCode, Url};
use tokio::io::AsyncWriteExt;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("can't reach the update server: {0}")]
    Unreachable(String),
    #[error("the update server answered {0}")]
    Status(u16),
    #[error("larger than announced")]
    TooLarge,
    #[error("the download stopped: {0}")]
    Interrupted(String),
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
}

pub trait Fetcher: Send + Sync + 'static {
    /// GET `url` into memory, at most `max` bytes.
    fn get(
        &self,
        url: &Url,
        max: usize,
    ) -> impl Future<Output = Result<Vec<u8>, FetchError>> + Send;

    /// Download `url` into `dest`, keeping what is already there. Fails with
    /// `TooLarge` past `expected` bytes and `Interrupted` short of them.
    /// `progress` gets the file's length as it grows.
    fn download(
        &self,
        url: &Url,
        dest: &Path,
        expected: u64,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> impl Future<Output = Result<(), FetchError>> + Send;
}

pub struct HttpFetcher {
    client: reqwest::Client,
}

impl HttpFetcher {
    pub fn new() -> Result<Self, FetchError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("waved/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| FetchError::Unreachable(e.to_string()))?;
        Ok(Self { client })
    }
}

fn unreachable(error: reqwest::Error) -> FetchError {
    FetchError::Unreachable(error.to_string())
}

impl Fetcher for HttpFetcher {
    async fn get(&self, url: &Url, max: usize) -> Result<Vec<u8>, FetchError> {
        let mut response = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(unreachable)?;
        if !response.status().is_success() {
            return Err(FetchError::Status(response.status().as_u16()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
            if body.len() + chunk.len() > max {
                return Err(FetchError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    async fn download(
        &self,
        url: &Url,
        dest: &Path,
        expected: u64,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), FetchError> {
        let mut have = tokio::fs::metadata(dest)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        if have > expected {
            tokio::fs::remove_file(dest).await?;
            have = 0;
        }
        if have == expected {
            progress(have);
            return Ok(());
        }
        let mut request = self.client.get(url.clone());
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut response = request.send().await.map_err(unreachable)?;
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).write(true);
        match response.status() {
            StatusCode::PARTIAL_CONTENT if have > 0 => {
                options.append(true);
            }
            status if status.is_success() => {
                options.truncate(true);
                have = 0;
            }
            StatusCode::RANGE_NOT_SATISFIABLE => {
                tokio::fs::remove_file(dest).await?;
                return Err(FetchError::Interrupted(
                    "the server refused to resume".into(),
                ));
            }
            status => return Err(FetchError::Status(status.as_u16())),
        }
        let mut file = options.open(dest).await?;
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(e) => {
                    file.flush().await?;
                    return Err(FetchError::Interrupted(e.to_string()));
                }
            };
            have += chunk.len() as u64;
            if have > expected {
                drop(file);
                tokio::fs::remove_file(dest).await?;
                return Err(FetchError::TooLarge);
            }
            file.write_all(&chunk).await?;
            progress(have);
        }
        file.flush().await?;
        file.sync_all().await?;
        if have < expected {
            return Err(FetchError::Interrupted("the server closed early".into()));
        }
        Ok(())
    }
}

/// Host fake: serves byte vectors by URL, can cut a download short once and
/// records where each download started.
#[derive(Clone, Default)]
pub struct FakeFetcher {
    inner: Arc<Mutex<FakeInner>>,
}

#[derive(Default)]
struct FakeInner {
    files: HashMap<String, Vec<u8>>,
    cut_after: Option<u64>,
    unreachable: bool,
    gets: Vec<String>,
    downloads: Vec<(String, u64)>,
}

impl FakeFetcher {
    pub fn serve(&self, url: &str, bytes: Vec<u8>) {
        self.inner.lock().unwrap().files.insert(url.into(), bytes);
    }

    /// The next download stops once the file holds `bytes` bytes.
    pub fn cut_next_download_at(&self, bytes: u64) {
        self.inner.lock().unwrap().cut_after = Some(bytes);
    }

    pub fn set_unreachable(&self, unreachable: bool) {
        self.inner.lock().unwrap().unreachable = unreachable;
    }

    pub fn gets(&self) -> Vec<String> {
        self.inner.lock().unwrap().gets.clone()
    }

    /// (url, offset it resumed from) per download.
    pub fn downloads(&self) -> Vec<(String, u64)> {
        self.inner.lock().unwrap().downloads.clone()
    }
}

impl Fetcher for FakeFetcher {
    async fn get(&self, url: &Url, max: usize) -> Result<Vec<u8>, FetchError> {
        let mut inner = self.inner.lock().unwrap();
        inner.gets.push(url.to_string());
        if inner.unreachable {
            return Err(FetchError::Unreachable("fake".into()));
        }
        let body = inner
            .files
            .get(url.as_str())
            .cloned()
            .ok_or(FetchError::Status(404))?;
        if body.len() > max {
            return Err(FetchError::TooLarge);
        }
        Ok(body)
    }

    async fn download(
        &self,
        url: &Url,
        dest: &Path,
        expected: u64,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), FetchError> {
        let (body, cut) = {
            let mut inner = self.inner.lock().unwrap();
            if inner.unreachable {
                return Err(FetchError::Unreachable("fake".into()));
            }
            let body = inner
                .files
                .get(url.as_str())
                .cloned()
                .ok_or(FetchError::Status(404))?;
            let have = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
            inner.downloads.push((url.to_string(), have));
            (body, inner.cut_after.take())
        };
        let have = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0) as usize;
        if body.len() as u64 > expected {
            return Err(FetchError::TooLarge);
        }
        let end = cut.map_or(body.len(), |cut| (cut as usize).min(body.len()));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dest)?;
        if end > have {
            // Two pieces, so progress moves the way it does over a network.
            let middle = have + (end - have) / 2;
            for piece in [&body[have..middle], &body[middle..end]] {
                io::Write::write_all(&mut file, piece)?;
                progress(std::fs::metadata(dest)?.len());
                tokio::task::yield_now().await;
            }
        }
        if end < body.len() {
            return Err(FetchError::Interrupted("fake cut".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[tokio::test]
    async fn the_fake_resumes_where_the_cut_left_off() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("p");
        let fake = FakeFetcher::default();
        let body: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        fake.serve("http://h/p", body.clone());
        fake.cut_next_download_at(300);
        let mut seen = Vec::new();
        let err = fake
            .download(&url("http://h/p"), &dest, 1000, &mut |n| seen.push(n))
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Interrupted(_)));
        assert_eq!(std::fs::read(&dest).unwrap().len(), 300);
        fake.download(&url("http://h/p"), &dest, 1000, &mut |n| seen.push(n))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(
            fake.downloads(),
            vec![("http://h/p".into(), 0), ("http://h/p".into(), 300)]
        );
        assert_eq!(*seen.last().unwrap(), 1000);
    }

    #[tokio::test]
    async fn the_fake_answers_like_a_server() {
        let fake = FakeFetcher::default();
        assert!(matches!(
            fake.get(&url("http://h/missing"), 10).await,
            Err(FetchError::Status(404))
        ));
        fake.serve("http://h/big", vec![0; 100]);
        assert!(matches!(
            fake.get(&url("http://h/big"), 10).await,
            Err(FetchError::TooLarge)
        ));
        fake.set_unreachable(true);
        assert!(matches!(
            fake.get(&url("http://h/big"), 1000).await,
            Err(FetchError::Unreachable(_))
        ));
    }
}
