//! `…/sandbox/{id}/files`: one box's filesystem, a range or a chunk per call. Moving a whole
//! file or directory is the caller's.

use bytes::Bytes;
use reqwest::header::{CONTENT_RANGE, CONTENT_TYPE, HeaderMap, RANGE};
use reqwest::{Method, StatusCode};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use super::sandbox_path;
use crate::error::{Error, failure, transport};
use crate::time_portal::TimePortal;

const FILES: &str = "/files";
const OCTETS: &str = "application/octet-stream";

/// What kind of file a path is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FileType {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// Anything else, including kinds a newer server names.
    #[serde(other)]
    Other,
}

/// One path's metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FileStat {
    /// Its absolute path; in a directory listing, its path relative to the listed directory.
    pub path: String,
    /// What kind of file it is.
    #[serde(rename = "type")]
    pub file_type: FileType,
    /// Its size in bytes.
    pub size: u64,
    /// Its permission bits, e.g. `0o644`.
    pub mode: u32,
    /// When it last changed, by the box's clock.
    #[serde(default)]
    pub mtime: Option<String>,
    /// Where a symlink points.
    #[serde(default)]
    pub link_target: Option<String>,
}

/// A page of a directory's entries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FileList {
    /// The entries.
    pub entries: Vec<FileStat>,
    /// Where the next page starts; `None` on the last page.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// `GET …/files/list`: one page of a directory, from `FileListRequest::new(path)` then its
/// fields.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileListRequest {
    /// The directory, absolute.
    pub path: String,
    /// Every level below it, not just its own entries.
    pub recursive: bool,
    /// Where the last page ended: its [`FileList::next_cursor`].
    pub cursor: Option<String>,
    /// The most entries on the page, 1 to 100.
    pub limit: Option<u32>,
}

impl FileListRequest {
    /// The entries of `path`.
    pub fn new(path: impl Into<String>) -> Self {
        FileListRequest {
            path: path.into(),
            recursive: false,
            cursor: None,
            limit: None,
        }
    }
}

/// Bytes of a file, from [`TimePortal::read_file`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileRange {
    /// Where `data` starts in the file.
    pub start: u64,
    /// The bytes; empty when the read starts at or past the end.
    pub data: Vec<u8>,
    /// The file's whole size, when the box said.
    pub total: Option<u64>,
}

impl TimePortal {
    /// Metadata of `path`, without following a final symlink.
    pub async fn stat_file(&self, id: &str, path: &str) -> Result<FileStat, Error> {
        let http = self
            .request(Method::GET, &sandbox_path(id, "/files/stat")?)
            .query(&[("path", path)]);
        self.client.json(http).await
    }

    /// One page of the entries under `request.path`.
    pub async fn list_files(&self, id: &str, request: &FileListRequest) -> Result<FileList, Error> {
        let mut query = vec![("path", request.path.clone())];
        if request.recursive {
            query.push(("recursive", true.to_string()));
        }
        if let Some(cursor) = &request.cursor {
            query.push(("cursor", cursor.clone()));
        }
        if let Some(limit) = request.limit {
            query.push(("limit", limit.to_string()));
        }
        let http = self.request(Method::GET, &sandbox_path(id, "/files/list")?);
        self.client.json(http.query(&query)).await
    }

    /// Bytes of `path` from `offset`: `len` of them, or with `None` as many as the box sends in
    /// one call (16 MiB). A read starting at or past the end comes back empty, not as an error.
    pub async fn read_file(
        &self,
        id: &str,
        path: &str,
        offset: u64,
        len: Option<u64>,
    ) -> Result<FileRange, Error> {
        let range = match len {
            Some(0) => {
                return Err(Error::InvalidArgument(
                    "a read must ask for at least one byte".into(),
                ));
            }
            Some(len) => format!("bytes={offset}-{}", offset.saturating_add(len - 1)),
            None => format!("bytes={offset}-"),
        };
        let http = self
            .request(Method::GET, &sandbox_path(id, FILES)?)
            .query(&[("path", path)])
            .header(RANGE, range);
        let resp = self.client.send_any(http).await?;
        let status = resp.status();
        let (start, total) = content_range(resp.headers());
        match status {
            StatusCode::RANGE_NOT_SATISFIABLE => Ok(FileRange {
                start: offset,
                data: Vec::new(),
                total,
            }),
            StatusCode::PARTIAL_CONTENT => Ok(FileRange {
                start: start.unwrap_or(offset),
                data: resp.bytes().await.map_err(transport)?.into(),
                total,
            }),
            StatusCode::OK => Ok(FileRange {
                start: 0,
                data: resp.bytes().await.map_err(transport)?.into(),
                total,
            }),
            _ if !status.is_success() => Err(failure(resp).await),
            _ => Err(Error::Decode(serde_json::Error::custom(format!(
                "{path}: a range read answered HTTP {status}"
            )))),
        }
    }

    /// Writes `data` (at most 8 MiB) into `path` at `offset`, and returns the file as it is
    /// after. Offset 0 replaces the file whole; a later offset writes into it, zero-filling any
    /// gap. `data` is shared, not copied, so a retry can send it again cheaply.
    pub async fn write_file(
        &self,
        id: &str,
        path: &str,
        offset: u64,
        data: impl Into<Bytes>,
    ) -> Result<FileStat, Error> {
        let http = self
            .request(Method::PUT, &sandbox_path(id, FILES)?)
            .query(&[("path", path), ("offset", &offset.to_string())])
            .header(CONTENT_TYPE, OCTETS)
            .body(data.into());
        self.client.json(http).await
    }

    /// Removes `path`; a directory only with `recursive`.
    pub async fn remove_file(&self, id: &str, path: &str, recursive: bool) -> Result<(), Error> {
        let mut query = vec![("path", path.to_owned())];
        if recursive {
            query.push(("recursive", true.to_string()));
        }
        let http = self.request(Method::DELETE, &sandbox_path(id, FILES)?);
        self.client.send(http.query(&query)).await?;
        Ok(())
    }
}

/// `Content-Range: bytes <first>-<last>/<size>` or `bytes */<size>`: the first byte and the size.
fn content_range(headers: &HeaderMap) -> (Option<u64>, Option<u64>) {
    let Some(value) = headers.get(CONTENT_RANGE).and_then(|v| v.to_str().ok()) else {
        return (None, None);
    };
    let Some((span, size)) = value.trim_start_matches("bytes ").rsplit_once('/') else {
        return (None, None);
    };
    let first = span
        .split_once('-')
        .and_then(|(first, _)| first.parse().ok());
    (first, size.parse().ok())
}
