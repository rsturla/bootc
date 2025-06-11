//! APIs for "unencapsulating" OSTree commits from container images
//!
//! This code only operates on container images that were created via
//! [`encapsulate`].
//!
//! # External dependency on container-image-proxy
//!
//! This code requires <https://github.com/cgwalters/container-image-proxy>
//! installed as a binary in $PATH.
//!
//! The rationale for this is that while there exist Rust crates to speak
//! the Docker distribution API, the Go library <https://github.com/containers/image/>
//! supports key things we want for production use like:
//!
//! - Image mirroring and remapping; effectively `man containers-registries.conf`
//!   For example, we need to support an administrator mirroring an ostree-container
//!   into a disconnected registry, without changing all the pull specs.
//! - Signing
//!
//! Additionally, the proxy "upconverts" manifests into OCI, so we don't need to care
//! about parsing the Docker manifest format (as used by most registries still).
//!
//! [`encapsulate`]: [`super::encapsulate()`]

// # Implementation
//
// First, we support explicitly fetching just the manifest: https://github.com/opencontainers/image-spec/blob/main/manifest.md
// This will give us information about the layers it contains, and crucially the digest (sha256) of
// the manifest is how higher level software can detect changes.
//
// Once we have the manifest, we expect it to point to a single `application/vnd.oci.image.layer.v1.tar+gzip` layer,
// which is exactly what is exported by the [`crate::tar::export`] process.

use crate::container::store::LayerProgress;

use super::*;
use containers_image_proxy::{ImageProxy, OpenedImage};
use fn_error_context::context;
use futures_util::{Future, FutureExt};
use oci_spec::image::{self as oci_image, Digest};
use std::io::Read;
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncBufRead, AsyncRead},
    sync::watch::{Receiver, Sender},
};
use tracing::instrument;

/// The legacy MIME type returned by the skopeo/(containers/storage) code
/// when we have local uncompressed docker-formatted image.
/// TODO: change the skopeo code to shield us from this correctly
const DOCKER_TYPE_LAYER_TAR: &str = "application/vnd.docker.image.rootfs.diff.tar";

type Progress = tokio::sync::watch::Sender<u64>;

/// A read wrapper that updates the download progress.
#[pin_project::pin_project]
#[derive(Debug)]
pub(crate) struct ProgressReader<T> {
    #[pin]
    pub(crate) reader: T,
    #[pin]
    pub(crate) progress: Arc<Mutex<Progress>>,
}

impl<T: AsyncRead> ProgressReader<T> {
    pub(crate) fn new(reader: T) -> (Self, Receiver<u64>) {
        let (progress, r) = tokio::sync::watch::channel(1);
        let progress = Arc::new(Mutex::new(progress));
        (ProgressReader { reader, progress }, r)
    }
}

impl<T: AsyncRead> AsyncRead for ProgressReader<T> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.project();
        let len = buf.filled().len();
        match this.reader.poll_read(cx, buf) {
            v @ std::task::Poll::Ready(Ok(_)) => {
                let progress = this.progress.lock().unwrap();
                let state = {
                    let mut state = *progress.borrow();
                    let newlen = buf.filled().len();
                    debug_assert!(newlen >= len);
                    let read = (newlen - len) as u64;
                    state += read;
                    state
                };
                // Ignore errors, if the caller disconnected from progress that's OK.
                let _ = progress.send(state);
                v
            }
            o => o,
        }
    }
}

async fn fetch_manifest_impl(
    proxy: &mut ImageProxy,
    imgref: &OstreeImageReference,
) -> Result<(oci_image::ImageManifest, oci_image::Digest)> {
    let oi = &proxy.open_image(&imgref.imgref.to_string()).await?;
    let (digest, manifest) = proxy.fetch_manifest(oi).await?;
    proxy.close_image(oi).await?;
    Ok((manifest, oci_image::Digest::from_str(digest.as_str())?))
}

/// Download the manifest for a target image and its sha256 digest.
#[context("Fetching manifest")]
pub async fn fetch_manifest(
    imgref: &OstreeImageReference,
) -> Result<(oci_image::ImageManifest, oci_image::Digest)> {
    let mut proxy = ImageProxy::new().await?;
    fetch_manifest_impl(&mut proxy, imgref).await
}

/// Download the manifest for a target image and its sha256 digest, as well as the image configuration.
#[context("Fetching manifest and config")]
pub async fn fetch_manifest_and_config(
    imgref: &OstreeImageReference,
) -> Result<(
    oci_image::ImageManifest,
    oci_image::Digest,
    oci_image::ImageConfiguration,
)> {
    let proxy = ImageProxy::new().await?;
    let oi = &proxy.open_image(&imgref.imgref.to_string()).await?;
    let (digest, manifest) = proxy.fetch_manifest(oi).await?;
    let digest = oci_image::Digest::from_str(&digest)?;
    let config = proxy.fetch_config(oi).await?;
    Ok((manifest, digest, config))
}

/// The result of an import operation
#[derive(Debug)]
pub struct Import {
    /// The ostree commit that was imported
    pub ostree_commit: String,
    /// The image digest retrieved
    pub image_digest: Digest,

    /// Any deprecation warning
    pub deprecated_warning: Option<String>,
}

/// Use this to process potential errors from a worker and a driver.
/// This is really a brutal hack around the fact that an error can occur
/// on either our side or in the proxy.  But if an error occurs on our
/// side, then we will close the pipe, which will *also* cause the proxy
/// to error out.
///
/// What we really want is for the proxy to tell us when it got an
/// error from us closing the pipe.  Or, we could store that state
/// on our side.  Both are slightly tricky, so we have this (again)
/// hacky thing where we just search for `broken pipe` in the error text.
///
/// Or to restate all of the above - what this function does is check
/// to see if the worker function had an error *and* if the proxy
/// had an error, but if the proxy's error ends in `broken pipe`
/// then it means the real only error is from the worker.
pub(crate) async fn join_fetch<T: std::fmt::Debug>(
    worker: impl Future<Output = Result<T>>,
    driver: impl Future<Output = Result<()>>,
) -> Result<T> {
    let (worker, driver) = tokio::join!(worker, driver);
    match (worker, driver) {
        (Ok(t), Ok(())) => Ok(t),
        (Err(worker), Err(driver)) => {
            let text = driver.root_cause().to_string();
            if text.ends_with("broken pipe") {
                tracing::trace!("Ignoring broken pipe failure from driver");
                Err(worker)
            } else {
                Err(worker.context(format!("proxy failure: {} and client error", text)))
            }
        }
        (Ok(_), Err(driver)) => Err(driver),
        (Err(worker), Ok(())) => Err(worker),
    }
}

/// Fetch a container image and import its embedded OSTree commit.
#[context("Importing {}", imgref)]
#[instrument(level = "debug", skip(repo))]
pub async fn unencapsulate(repo: &ostree::Repo, imgref: &OstreeImageReference) -> Result<Import> {
    let importer = super::store::ImageImporter::new(repo, imgref, Default::default()).await?;
    importer.unencapsulate().await
}

pub(crate) struct Decompressor {
    inner: Box<dyn Read + Send + 'static>,
    finished: bool,
}

impl Read for Decompressor {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Drop for Decompressor {
    fn drop(&mut self) {
        if self.finished {
            return;
        }

        // We really should not get here; users are required to call
        // `finish()` to clean up the stream.  But we'll give
        // best-effort to clean things up nonetheless.  If things go
        // wrong, then panic, because we're in a bad state and it's
        // likely that we end up with a broken pipe error or a
        // deadlock.
        self._finish().expect("Decompressor::finish MUST be called")
    }
}

impl Decompressor {
    /// Create a decompressor for this MIME type, given a stream of input.
    pub(crate) fn new(
        media_type: &oci_image::MediaType,
        src: impl Read + Send + 'static,
    ) -> Result<Self> {
        let r: Box<dyn std::io::Read + Send + 'static> = match media_type {
            oci_image::MediaType::ImageLayerZstd => {
                Box::new(zstd::stream::read::Decoder::new(src)?)
            }
            oci_image::MediaType::ImageLayerGzip => Box::new(flate2::bufread::GzDecoder::new(
                std::io::BufReader::new(src),
            )),
            oci_image::MediaType::ImageLayer => Box::new(src),
            oci_image::MediaType::Other(t) if t.as_str() == DOCKER_TYPE_LAYER_TAR => Box::new(src),
            o => anyhow::bail!("Unhandled layer type: {}", o),
        };
        Ok(Self {
            inner: r,
            finished: false,
        })
    }

    pub(crate) fn finish(mut self) -> Result<()> {
        self._finish()
    }

    fn _finish(&mut self) -> Result<()> {
        self.finished = true;

        // We need to make sure to flush out the decompressor and/or
        // tar stream here.  For tar, we might not read through the
        // entire stream, because the archive has zero-block-markers
        // at the end; or possibly because the final entry is filtered
        // in filter_tar so we don't advance to read the data.  For
        // decompressor, zstd:chunked layers will have
        // metadata/skippable frames at the end of the stream.  That
        // data isn't relevant to the tar stream, but if we don't read
        // it here then on the skopeo proxy we'll block trying to
        // write the end of the stream.  That in turn will block our
        // client end trying to call FinishPipe, and we end up
        // deadlocking ourselves through skopeo.
        //
        // https://github.com/bootc-dev/bootc/issues/1204

        let mut sink = std::io::sink();
        let n = std::io::copy(&mut self.inner, &mut sink)?;

        if n > 0 {
            tracing::debug!("Read extra {n} bytes at end of decompressor stream");
        }

        Ok(())
    }
}

/// A wrapper for [`get_blob`] which fetches a layer and decompresses it.
pub(crate) async fn fetch_layer<'a>(
    proxy: &'a ImageProxy,
    img: &OpenedImage,
    manifest: &oci_image::ImageManifest,
    layer: &'a oci_image::Descriptor,
    progress: Option<&'a Sender<Option<store::LayerProgress>>>,
    layer_info: Option<&Vec<containers_image_proxy::ConvertedLayerInfo>>,
    transport_src: Transport,
) -> Result<(
    Box<dyn AsyncBufRead + Send + Unpin>,
    impl Future<Output = Result<()>> + 'a,
    oci_image::MediaType,
)> {
    use futures_util::future::Either;
    tracing::debug!("fetching {}", layer.digest());
    let layer_index = manifest.layers().iter().position(|x| x == layer).unwrap();
    let (blob, driver, size);
    let media_type: oci_image::MediaType;
    match transport_src {
        Transport::ContainerStorage => {
            let layer_info = layer_info
                .ok_or_else(|| anyhow!("skopeo too old to pull from containers-storage"))?;
            let n_layers = layer_info.len();
            let layer_blob = layer_info.get(layer_index).ok_or_else(|| {
                anyhow!("blobid position {layer_index} exceeds diffid count {n_layers}")
            })?;
            size = layer_blob.size;
            media_type = layer_blob.media_type.clone();
            (blob, driver) = proxy.get_blob(img, &layer_blob.digest, size).await?;
        }
        _ => {
            size = layer.size();
            media_type = layer.media_type().clone();
            (blob, driver) = proxy.get_blob(img, layer.digest(), size).await?;
        }
    };

    let driver = async { driver.await.map_err(Into::into) };

    if let Some(progress) = progress {
        let (readprogress, mut readwatch) = ProgressReader::new(blob);
        let readprogress = tokio::io::BufReader::new(readprogress);
        let readproxy = async move {
            while let Ok(()) = readwatch.changed().await {
                let fetched = readwatch.borrow_and_update();
                let status = LayerProgress {
                    layer_index,
                    fetched: *fetched,
                    total: size,
                };
                progress.send_replace(Some(status));
            }
        };
        let reader = Box::new(readprogress);
        let driver = futures_util::future::join(readproxy, driver).map(|r| r.1);
        Ok((reader, Either::Left(driver), media_type))
    } else {
        Ok((Box::new(blob), Either::Right(driver), media_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenPipe;

    impl Read for BrokenPipe {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            std::io::Result::Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    #[should_panic(expected = "Decompressor::finish MUST be called")]
    fn test_drop_decompressor_with_finish_error_should_panic() {
        let broken = BrokenPipe;
        let d = Decompressor::new(&oci_image::MediaType::ImageLayer, broken).unwrap();
        drop(d)
    }

    #[test]
    fn test_drop_decompressor_with_successful_finish() {
        let empty = std::io::empty();
        let d = Decompressor::new(&oci_image::MediaType::ImageLayer, empty).unwrap();
        drop(d)
    }
}
