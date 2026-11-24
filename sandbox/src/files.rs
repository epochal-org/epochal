//! A box's filesystem. Files move in ranges, so no request carries more than a chunk; directories
//! move as a tar through the box's own `tar`. Nothing uploaded dates the host: entries carry no
//! mtime or owner and the box extracts with its own clock.

use std::fs;
use std::io::{self, Seek, SeekFrom};
use std::path::{Component, Path};

use bytes::Bytes;
use epochal_api::Error as ApiError;
use epochal_api::time_portal::sandbox::FileStat;
use futures_util::{StreamExt, TryStreamExt, stream};
use tar::{Archive, Builder, Entry, EntryType, HeaderMode};
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::task::spawn_blocking;

use crate::retry::{random, retry};
use crate::sandbox::Sandbox;
use crate::{Error, invalid};

const WRITE_CHUNK: u64 = 8 << 20;
/// Calls one file holds open at once past its first: enough to overlap each range's round trip.
const IN_FLIGHT: usize = 4;
const EXTRACT: &str =
    r#"mkdir -p -- "$2" && tar -xmf "$1" --no-same-owner -C "$2"; s=$?; rm -f -- "$1"; exit $s"#;
/// `$3` keeps the archive out of itself when it lands under `$1`; a failed one is removed.
const ARCHIVE: &str =
    r#"tar -cf "$2" --exclude="$3" -C "$1" . || { s=$?; rm -f -- "$2"; exit $s; }"#;

/// A box's filesystem, from [`Sandbox::files`]. Paths in the box are absolute.
#[derive(Clone, Copy, Debug)]
pub struct Files<'a> {
    pub(crate) sandbox: &'a Sandbox,
}

impl Files<'_> {
    /// Metadata of `path`, without following a final symlink.
    pub async fn stat(&self, path: &str) -> Result<FileStat, Error> {
        let (sandbox, path) = (self.sandbox, absolute(path)?);
        Ok(retry(|| sandbox.portal.stat_file(sandbox.id(), path)).await?)
    }

    /// Copies the local file `local` to `path`, replacing it and creating missing parents.
    pub async fn upload_file(&self, local: impl AsRef<Path>, path: &str) -> Result<(), Error> {
        let path = absolute(path)?;
        self.write_from(path, File::open(local).await?).await
    }

    /// Copies the file at `path` to `local`.
    pub async fn download_file(&self, path: &str, local: impl AsRef<Path>) -> Result<(), Error> {
        let path = absolute(path)?;
        self.stat(path).await?;
        let mut file = File::create(local).await?;
        self.read_to(path, &mut file).await?;
        file.flush().await?;
        Ok(())
    }

    /// Copies the tree under the local directory `local` into `path`, replacing files there.
    pub async fn upload_dir(&self, local: impl AsRef<Path>, path: &str) -> Result<(), Error> {
        let path = absolute(path)?;
        let local = local.as_ref().to_owned();
        let tarball = spawn_blocking(move || tar_tree(&local))
            .await
            .map_err(io::Error::other)??;
        let temp = temp_name("u");
        if let Err(e) = self.write_from(&temp, File::from_std(tarball)).await {
            let sandbox = self.sandbox;
            let _ = retry(|| sandbox.portal.remove_file(sandbox.id(), &temp, false)).await;
            return Err(e);
        }
        self.sh("unpacking the upload", EXTRACT, [&temp, path, ""])
            .await
    }

    /// Copies the tree under `path` into the local directory `local`, made if missing. Links
    /// that could point outside `local` are skipped.
    pub async fn download_dir(&self, path: &str, local: impl AsRef<Path>) -> Result<(), Error> {
        let path = absolute(path)?;
        let local = local.as_ref().to_owned();
        let temp = temp_name("d");
        let inside = archive_within(path, &temp);
        self.sh("archiving the download", ARCHIVE, [path, &temp, &inside])
            .await?;
        let mut spool = File::from_std(tempfile::tempfile()?);
        let fetched = self.read_to(&temp, &mut spool).await;
        let sandbox = self.sandbox;
        let removed = retry(|| sandbox.portal.remove_file(sandbox.id(), &temp, false)).await;
        fetched?;
        // The name is random, so a retry that finds it gone found the first attempt's removal.
        if !matches!(&removed, Err(ApiError::Api { status: 404, .. })) {
            removed?;
        }
        let mut spool = spool.into_std().await;
        spool.seek(SeekFrom::Start(0))?;
        spawn_blocking(move || untar(spool, &local))
            .await
            .map_err(io::Error::other)?
    }

    async fn sh(&self, step: &str, script: &str, args: [&str; 3]) -> Result<(), Error> {
        let argv = ["sh", "-c", script, "sh", args[0], args[1], args[2]];
        let output = self.sandbox.command(argv).output().await?;
        if output.success() {
            return Ok(());
        }
        Err(Error::CommandFailed {
            step: step.to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }

    /// Streams the file at `path` into `out` in order. The first read takes as much as the box
    /// sends; once it names the size, the rest come in ranges of that length, [`IN_FLIGHT`] at a
    /// time, and a short one restarts them where it ended. Without a size, reads go one by one.
    async fn read_to(&self, path: &str, out: &mut (impl AsyncWrite + Unpin)) -> Result<(), Error> {
        let sandbox = self.sandbox;
        let read = |offset: u64, len: Option<u64>| async move {
            let range = retry(|| sandbox.portal.read_file(sandbox.id(), path, offset, len)).await;
            (offset, len, range)
        };
        let (mut offset, mut total, mut chunk) = (0u64, None::<u64>, None::<u64>);
        loop {
            let spans: Box<dyn Iterator<Item = (u64, Option<u64>)> + Send> = match (total, chunk) {
                (Some(total), Some(chunk)) => Box::new(
                    (offset..total)
                        .step_by(chunk as usize)
                        .map(move |at| (at, Some(chunk.min(total - at)))),
                ),
                _ => Box::new(std::iter::once((offset, None))),
            };
            let mut ranges = stream::iter(spans)
                .map(|(at, len)| read(at, len))
                .buffered(IN_FLIGHT);
            while let Some((at, len, range)) = ranges.next().await {
                let range = range?;
                if range.start != at {
                    return Err(Error::Unexpected(format!(
                        "{path}: asked for byte {at}, got byte {}",
                        range.start
                    )));
                }
                out.write_all(&range.data).await?;
                let got = range.data.len() as u64;
                (offset, total) = (offset + got, range.total);
                if got == 0 || total.is_some_and(|total| offset >= total) {
                    return Ok(());
                }
                chunk = chunk.or(Some(got));
                if len.is_some_and(|len| got < len) {
                    break;
                }
            }
        }
    }

    /// Writes `data` to `path` in chunks: the first, at offset 0, replaces the file and goes
    /// alone; the rest land at their offsets, up to [`IN_FLIGHT`] at once, each read only once a
    /// call is free for it.
    async fn write_from(&self, path: &str, mut data: impl AsyncRead + Unpin) -> Result<(), Error> {
        let sandbox = self.sandbox;
        let write = |offset: u64, chunk: Bytes| async move {
            retry(|| {
                sandbox
                    .portal
                    .write_file(sandbox.id(), path, offset, chunk.clone())
            })
            .await
            .map_err(Error::from)
        };
        let first = next_chunk(&mut data).await?;
        let offset = first.len() as u64;
        write(0, first).await?;
        let chunks = stream::try_unfold((data, offset), |(mut data, offset)| async move {
            let chunk = next_chunk(&mut data).await?;
            let next = offset + chunk.len() as u64;
            Ok::<_, Error>((!chunk.is_empty()).then_some(((offset, chunk), (data, next))))
        });
        chunks
            .map_ok(|(offset, chunk)| write(offset, chunk))
            .try_buffer_unordered(IN_FLIGHT)
            .try_for_each(|_| async { Ok(()) })
            .await
    }
}

async fn next_chunk(data: &mut (impl AsyncRead + Unpin)) -> io::Result<Bytes> {
    let mut chunk = Vec::new();
    data.take(WRITE_CHUNK).read_to_end(&mut chunk).await?;
    Ok(chunk.into())
}

/// A scratch path in the box. The name is random, never time-ordered, so nothing in the box
/// learns today's date from it.
fn temp_name(kind: &str) -> String {
    format!("/tmp/.{kind}-{:016x}.tar", random())
}

/// Box paths also reach shell commands as arguments, where a leading `/` keeps one from being
/// read as an option.
fn absolute(path: &str) -> Result<&str, Error> {
    if path.starts_with('/') {
        Ok(path)
    } else {
        Err(invalid(format!("box path {path:?} must be absolute")))
    }
}

/// `temp` as `tar -C src .` names it; outside `src`, a name that matches nothing.
fn archive_within(src: &str, temp: &str) -> String {
    let src = src.trim_end_matches('/');
    match temp.strip_prefix(src).and_then(|r| r.strip_prefix('/')) {
        Some(rel) => format!("./{rel}"),
        None => temp.to_owned(),
    }
}

fn tar_tree(root: &Path) -> Result<fs::File, Error> {
    if !fs::metadata(root)?.is_dir() {
        return Err(invalid(format!("{} is not a directory", root.display())));
    }
    let mut builder = Builder::new(tempfile::tempfile()?);
    builder.mode(HeaderMode::Deterministic);
    builder.follow_symlinks(false);
    builder.append_dir_all(".", root)?;
    let mut file = builder.into_inner()?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

/// Unpacks like `Archive::unpack`, directories last so a read-only one is filled first, with the
/// box's modes masked to the user's own and only links whose targets stay below their entry.
fn untar(tarball: fs::File, dest: &Path) -> Result<(), Error> {
    fs::create_dir_all(dest)?;
    let dest = dest.canonicalize()?;
    let mut archive = Archive::new(tarball);
    archive.set_preserve_mtime(false);
    archive.set_mask(0o022);
    let mut dirs = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        match entry.header().entry_type() {
            EntryType::Directory => dirs.push(entry),
            EntryType::Symlink | EntryType::Link if !contained(entry.link_name()?.as_deref()) => {}
            EntryType::Regular | EntryType::Continuous | EntryType::Symlink | EntryType::Link => {
                unpack(&mut entry, &dest)?
            }
            _ => {}
        }
    }
    dirs.sort_by(|a, b| b.path_bytes().cmp(&a.path_bytes()));
    for mut dir in dirs {
        unpack(&mut dir, &dest)?;
    }
    Ok(())
}

/// A relative target with no `..`: it can only name something below the link.
fn contained(target: Option<&Path>) -> bool {
    target.is_some_and(|t| {
        t.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    })
}

fn unpack(entry: &mut Entry<'_, fs::File>, dest: &Path) -> Result<(), Error> {
    if entry.unpack_in(dest)? {
        return Ok(());
    }
    Err(Error::Unexpected(format!(
        "archive entry {} escapes {}",
        entry.path_bytes().escape_ascii(),
        dest.display()
    )))
}
