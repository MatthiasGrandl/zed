use crate::{AppContext, ImageData, ImageId, SharedUri, Task};
use collections::HashMap;
use futures::{future::Shared, AsyncReadExt, FutureExt, TryFutureExt};
use image::ImageError;
use parking_lot::Mutex;
use std::sync::Arc;
use std::{fs, path::PathBuf};
use thiserror::Error;
use util::http::{self, HttpClient};

pub use image::ImageFormat;

#[derive(PartialEq, Eq, Hash, Clone)]
pub(crate) struct RenderImageParams {
    pub(crate) image_id: ImageId,
}

#[derive(Debug, Error, Clone)]
pub enum ImageCacheError {
    #[error("http error: {0}")]
    Client(#[from] http::Error),
    #[error("IO error: {0}")]
    Io(Arc<std::io::Error>),
    #[error("unexpected http status: {status}, body: {body}")]
    BadStatus {
        status: http::StatusCode,
        body: String,
    },
    #[error("image error: {0}")]
    Image(Arc<ImageError>),
    #[error("svg error: {0}")]
    Usvg(Arc<resvg::usvg::Error>),
}

impl From<std::io::Error> for ImageCacheError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(Arc::new(error))
    }
}

impl From<ImageError> for ImageCacheError {
    fn from(error: ImageError) -> Self {
        Self::Image(Arc::new(error))
    }
}

impl From<resvg::usvg::Error> for ImageCacheError {
    fn from(error: resvg::usvg::Error) -> Self {
        Self::Usvg(Arc::new(error))
    }
}

pub(crate) struct ImageCache {
    client: Arc<dyn HttpClient>,
    images: Arc<Mutex<HashMap<UriOrPath, FetchImageTask>>>,
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub(crate) enum UriOrPath {
    Uri(SharedUri),
    Path(Arc<PathBuf>),
}

impl From<SharedUri> for UriOrPath {
    fn from(value: SharedUri) -> Self {
        Self::Uri(value)
    }
}

impl From<Arc<PathBuf>> for UriOrPath {
    fn from(value: Arc<PathBuf>) -> Self {
        Self::Path(value)
    }
}

pub type FetchImageTask = Shared<Task<Result<Arc<ImageData>, ImageCacheError>>>;

impl ImageCache {
    pub fn new(client: Arc<dyn HttpClient>) -> Self {
        ImageCache {
            client,
            images: Default::default(),
        }
    }

    pub fn get(&self, uri_or_path: impl Into<UriOrPath>, cx: &AppContext) -> FetchImageTask {
        let uri_or_path = uri_or_path.into();
        let mut images = self.images.lock();

        match images.get(&uri_or_path) {
            Some(future) => future.clone(),
            None => {
                let client = self.client.clone();
                let future = cx
                    .background_executor()
                    .spawn(
                        {
                            let uri_or_path = uri_or_path.clone();
                            async move {
                                let body = match uri_or_path {
                                    UriOrPath::Path(uri) => fs::read(uri.as_ref())?,
                                    UriOrPath::Uri(uri) => {
                                        let mut response =
                                            client.get(uri.as_ref(), ().into(), true).await?;
                                        let mut body = Vec::new();
                                        response.body_mut().read_to_end(&mut body).await?;
                                        if !response.status().is_success() {
                                            return Err(ImageCacheError::BadStatus {
                                                status: response.status(),
                                                body: String::from_utf8_lossy(&body).into_owned(),
                                            });
                                        }
                                        body
                                    }
                                };
                                Ok(Arc::new(ImageData::try_from_bytes(&body)?))
                            }
                        }
                        .map_err({
                            let uri_or_path = uri_or_path.clone();
                            move |error| {
                                log::log!(log::Level::Error, "{:?} {:?}", &uri_or_path, &error);
                                error
                            }
                        }),
                    )
                    .shared();

                images.insert(uri_or_path, future.clone());
                future
            }
        }
    }
}
