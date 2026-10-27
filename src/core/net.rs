//! HTTP 下载：重试、SHA-1 校验、多线程并发、进度上报。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde::de::DeserializeOwned;
use sha1::{Digest, Sha1};

use super::error::{CoreError, Result, io_err};
use super::hash::{hex_encode, sha1_matches};
use super::progress::Progress;

const USER_AGENT: &str = concat!("NeutrinoMCL/", env!("CARGO_PKG_VERSION"));

/// 单个文件的下载结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadOutcome {
    /// 本地文件已存在且校验通过，跳过。
    Cached,
    /// 实际下载了。
    Downloaded,
}

/// 一个待下载文件。
#[derive(Debug, Clone)]
pub struct DownloadJob {
    pub url: String,
    pub dest: PathBuf,
    pub sha1: Option<String>,
    /// 预期大小，仅在无 sha1 时用于判断缓存是否可用，也用于进度总量。
    pub size: u64,
}

impl DownloadJob {
    /// 构造一个下载任务。
    pub fn new(
        url: impl Into<String>,
        dest: impl Into<PathBuf>,
        sha1: Option<String>,
        size: u64,
    ) -> Self {
        Self {
            url: url.into(),
            dest: dest.into(),
            sha1,
            size,
        }
    }

    /// 这批任务的总字节数。
    pub fn total_size(jobs: &[Self]) -> u64 {
        jobs.iter().map(|job| job.size).sum()
    }
}

/// 一批下载的统计。
#[derive(Debug, Default, Clone, Copy)]
pub struct DownloadStats {
    pub files: usize,
    pub cached: usize,
    pub bytes: u64,
}

/// HTTP 下载器。内部持有一个连接池可复用的 agent，可安全地跨线程共享。
#[derive(Clone)]
pub struct Downloader {
    agent: ureq::Agent,
    progress: Progress,
    retries: u32,
}

impl Downloader {
    /// 创建下载器。
    pub fn new(progress: Progress) -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(15)))
            // 单次读超时：大文件下载时只要持续有数据就不会中断。
            .timeout_per_read(Some(Duration::from_secs(60)))
            .build()
            .new_agent();

        Self {
            agent,
            progress,
            retries: 3,
        }
    }

    /// 设置失败重试次数（不含首次尝试）。
    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// 取回原始字节。
    pub fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        self.get_bytes_with_headers(url, &[])
    }

    /// 带自定义请求头取回原始字节（例如 CurseForge 的 `x-api-key`）。
    pub fn get_bytes_with_headers(&self, url: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>> {
        // 资源索引 JSON 可能有好几 MB，默认 10 MB 上限太紧，这里放宽到 64 MB。
        const MAX_BYTES: u64 = 64 * 1024 * 1024;

        self.retry(|| {
            let mut response = self.request(url, headers)?;
            response
                .body_mut()
                .with_config()
                .limit(MAX_BYTES)
                .read_to_vec()
                .map_err(|err| CoreError::Network(format!("读取 {url} 的响应失败：{err}")))
        })
    }

    /// 取回并解析 JSON。
    pub fn get_json<T: DeserializeOwned>(&self, url: &str, context: &str) -> Result<T> {
        self.get_json_with_headers(url, context, &[])
    }

    /// 带自定义请求头取回并解析 JSON。
    pub fn get_json_with_headers<T: DeserializeOwned>(
        &self,
        url: &str,
        context: &str,
        headers: &[(&str, &str)],
    ) -> Result<T> {
        let bytes = self.get_bytes_with_headers(url, headers)?;
        serde_json::from_slice(&bytes).map_err(|source| CoreError::Json {
            context: context.to_string(),
            source,
        })
    }

    /// 下载单个文件；本地已有且校验通过时直接跳过。
    pub fn download(
        &self,
        url: &str,
        dest: &Path,
        sha1: Option<&str>,
        size: u64,
    ) -> Result<DownloadOutcome> {
        let cached = match sha1 {
            Some(expected) => sha1_matches(dest, expected),
            None => {
                size > 0
                    && fs::metadata(dest)
                        .map(|meta| meta.len() == size)
                        .unwrap_or(false)
            }
        };

        if cached {
            self.progress.advance(size);
            return Ok(DownloadOutcome::Cached);
        }

        if let Some(parent) = dest.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|err| io_err(parent, err))?;
        }

        self.retry(|| self.fetch_to_file(url, dest, sha1))?;
        Ok(DownloadOutcome::Downloaded)
    }

    /// 并发下载一批文件，任意一个失败即中止其余任务。
    pub fn download_all(&self, jobs: &[DownloadJob], threads: usize) -> Result<DownloadStats> {
        if jobs.is_empty() {
            return Ok(DownloadStats::default());
        }

        let workers = threads.clamp(1, 32).min(jobs.len());
        let cursor = AtomicUsize::new(0);
        let failure: Mutex<Option<CoreError>> = Mutex::new(None);
        let stats: Mutex<DownloadStats> = Mutex::new(DownloadStats::default());

        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let failed = failure.lock().map(|guard| guard.is_some()).unwrap_or(true);
                        if failed {
                            break;
                        }

                        let position = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(position) else {
                            break;
                        };

                        match self.download(&job.url, &job.dest, job.sha1.as_deref(), job.size) {
                            Ok(outcome) => {
                                if let Ok(mut guard) = stats.lock() {
                                    guard.files += 1;
                                    guard.bytes += job.size;
                                    if outcome == DownloadOutcome::Cached {
                                        guard.cached += 1;
                                    }
                                }
                            }
                            Err(err) => {
                                if let Ok(mut guard) = failure.lock()
                                    && guard.is_none()
                                {
                                    *guard = Some(err);
                                }
                                break;
                            }
                        }
                    }
                });
            }
        });

        if let Some(err) = failure.into_inner().ok().flatten() {
            return Err(err);
        }

        Ok(stats.into_inner().unwrap_or_default())
    }

    fn request(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let mut request = self.agent.get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }

        match request.call() {
            Ok(response) => Ok(response),
            Err(ureq::Error::StatusCode(status)) => Err(CoreError::HttpStatus {
                status,
                url: url.to_string(),
            }),
            Err(err) => Err(CoreError::Network(format!("{err}（{url}）"))),
        }
    }

    fn fetch_to_file(&self, url: &str, dest: &Path, sha1: Option<&str>) -> Result<()> {
        let response = self.request(url, &[])?;

        let mut tmp_name = dest.as_os_str().to_owned();
        tmp_name.push(".part");
        let tmp = PathBuf::from(tmp_name);

        let mut hasher = Sha1::new();
        {
            let mut reader = self
                .progress
                .counting_reader(response.into_body().into_reader());
            let mut file = File::create(&tmp).map_err(|err| io_err(&tmp, err))?;
            let mut buffer = [0u8; 64 * 1024];

            loop {
                let read = reader
                    .read(&mut buffer)
                    .map_err(|err| CoreError::Network(format!("下载 {url} 时中断：{err}")))?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                file.write_all(&buffer[..read])
                    .map_err(|err| io_err(&tmp, err))?;
            }

            file.flush().map_err(|err| io_err(&tmp, err))?;
        }

        let actual = hex_encode(&hasher.finalize());
        if let Some(expected) = sha1
            && !actual.eq_ignore_ascii_case(expected.trim())
        {
            let _ = fs::remove_file(&tmp);
            return Err(CoreError::HashMismatch {
                path: dest.to_path_buf(),
                expected: expected.to_string(),
                actual,
            });
        }

        // Windows 的 rename 可以覆盖已存在文件，这里先删是为了兼容个别只读场景。
        if dest.exists() {
            let _ = fs::remove_file(dest);
        }
        fs::rename(&tmp, dest).map_err(|err| io_err(dest, err))?;
        Ok(())
    }

    fn retry<T>(&self, mut operation: impl FnMut() -> Result<T>) -> Result<T> {
        let mut delay = Duration::from_millis(400);
        let mut attempt = 0;

        loop {
            match operation() {
                Ok(value) => return Ok(value),
                Err(err) => {
                    if attempt >= self.retries || !is_retryable(&err) {
                        return Err(err);
                    }
                    attempt += 1;
                    std::thread::sleep(delay);
                    delay = (delay * 2).min(Duration::from_secs(8));
                }
            }
        }
    }
}

fn is_retryable(err: &CoreError) -> bool {
    match err {
        CoreError::HttpStatus { status, .. } => *status >= 500 || *status == 429,
        CoreError::Network(_) => true,
        // 校验失败通常是传输被截断或镜像返回了错误页面，重下一次可能就好了
        CoreError::HashMismatch { .. } => true,
        _ => false,
    }
}

/// 按 RFC 3986 对查询参数 / URL 片段做百分号编码。
pub fn encode_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_size_sums_jobs() {
        let jobs = vec![
            DownloadJob::new("https://a", "a.jar", None, 100),
            DownloadJob::new("https://b", "b.jar", None, 250),
        ];
        assert_eq!(DownloadJob::total_size(&jobs), 350);
        assert_eq!(DownloadJob::total_size(&[]), 0);
    }

    #[test]
    fn client_errors_are_not_retried() {
        assert!(!is_retryable(&CoreError::HttpStatus {
            status: 404,
            url: "https://a".to_string(),
        }));
        assert!(is_retryable(&CoreError::HttpStatus {
            status: 503,
            url: "https://a".to_string(),
        }));
        assert!(is_retryable(&CoreError::Network("timed out".to_string())));
    }

    #[test]
    fn cached_file_is_reused_without_network() {
        let dir = std::env::temp_dir().join("neutrino-downloader-test");
        let _ = fs::create_dir_all(&dir);
        let dest = dir.join("payload.bin");
        fs::write(&dest, b"abc").unwrap();

        let expected = super::super::hash::sha1_bytes(b"abc");
        let downloader = Downloader::new(Progress::silent());
        // URL 故意写成不可达地址：只要缓存命中就不应该发起请求
        let outcome = downloader
            .download("https://example.invalid/nope", &dest, Some(&expected), 3)
            .unwrap();
        assert_eq!(outcome, DownloadOutcome::Cached);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn encodes_query_components() {
        assert_eq!(encode_component("1.21.1"), "1.21.1");
        assert_eq!(
            encode_component(r#"[["versions:1.21.1"]]"#),
            "%5B%5B%22versions%3A1.21.1%22%5D%5D"
        );
        assert_eq!(encode_component("a b&c"), "a%20b%26c");
    }
}
