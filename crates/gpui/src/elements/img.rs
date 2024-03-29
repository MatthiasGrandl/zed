use std::hash::Hasher;
use std::path::PathBuf;
use std::sync::Arc;
use std::{fs, hash::Hash};

use crate::{
    hash, point, px, size, svg_fontdb, AbsoluteLength, AppContext, Asset, Bounds, DefiniteLength,
    DevicePixels, Element, ElementContext, Hitbox, ImageData, InteractiveElement, Interactivity,
    IntoElement, LayoutId, Length, Pixels, SharedUri, Size, StyleRefinement, Styled, UriOrPath,
};
use futures::{AsyncReadExt, Future};
use image::ImageError;
#[cfg(target_os = "macos")]
use media::core_video::CVImageBuffer;
use thiserror::Error;
use util::{http, ResultExt};

pub use image::ImageFormat;

/// A source of image content.
#[derive(Clone, Debug)]
pub enum ImageSource {
    /// Image content will be loaded from provided URI at render time.
    Uri(SharedUri),
    /// Image content will be loaded from the provided file at render time.
    File(Arc<PathBuf>),
    /// Cached image data
    Data(Arc<ImageData>),
    // TODO: move surface definitions into mac platform module
    /// A CoreVideo image buffer
    #[cfg(target_os = "macos")]
    Surface(CVImageBuffer),
}

impl From<SharedUri> for ImageSource {
    fn from(value: SharedUri) -> Self {
        Self::Uri(value)
    }
}

impl From<&'static str> for ImageSource {
    fn from(uri: &'static str) -> Self {
        Self::Uri(uri.into())
    }
}

impl From<String> for ImageSource {
    fn from(uri: String) -> Self {
        Self::Uri(uri.into())
    }
}

impl From<Arc<PathBuf>> for ImageSource {
    fn from(value: Arc<PathBuf>) -> Self {
        Self::File(value)
    }
}

impl From<PathBuf> for ImageSource {
    fn from(value: PathBuf) -> Self {
        Self::File(value.into())
    }
}

impl From<Arc<ImageData>> for ImageSource {
    fn from(value: Arc<ImageData>) -> Self {
        Self::Data(value)
    }
}

#[cfg(target_os = "macos")]
impl From<CVImageBuffer> for ImageSource {
    fn from(value: CVImageBuffer) -> Self {
        Self::Surface(value)
    }
}

/// An image element.
pub struct Img {
    interactivity: Interactivity,
    source: ImageSource,
    grayscale: bool,
    object_fit: ObjectFit,
}

/// Create a new image element.
pub fn img(source: impl Into<ImageSource>) -> Img {
    Img {
        interactivity: Interactivity::default(),
        source: source.into(),
        grayscale: false,
        object_fit: ObjectFit::Contain,
    }
}

/// How to fit the image into the bounds of the element.
pub enum ObjectFit {
    /// The image will be stretched to fill the bounds of the element.
    Fill,
    /// The image will be scaled to fit within the bounds of the element.
    Contain,
    /// The image will be scaled to cover the bounds of the element.
    Cover,
    /// The image will maintain its original size.
    None,
}

impl ObjectFit {
    /// Get the bounds of the image within the given bounds.
    pub fn get_bounds(
        &self,
        bounds: Bounds<Pixels>,
        image_size: Size<DevicePixels>,
    ) -> Bounds<Pixels> {
        let image_size = image_size.map(|dimension| Pixels::from(u32::from(dimension)));
        let image_ratio = image_size.width / image_size.height;
        let bounds_ratio = bounds.size.width / bounds.size.height;

        match self {
            ObjectFit::Fill => bounds,
            ObjectFit::Contain => {
                let new_size = if bounds_ratio > image_ratio {
                    size(
                        image_size.width * (bounds.size.height / image_size.height),
                        bounds.size.height,
                    )
                } else {
                    size(
                        bounds.size.width,
                        image_size.height * (bounds.size.width / image_size.width),
                    )
                };

                Bounds {
                    origin: point(
                        bounds.origin.x + (bounds.size.width - new_size.width) / 2.0,
                        bounds.origin.y + (bounds.size.height - new_size.height) / 2.0,
                    ),
                    size: new_size,
                }
            }
            ObjectFit::Cover => {
                let new_size = if bounds_ratio > image_ratio {
                    size(
                        bounds.size.width,
                        image_size.height * (bounds.size.width / image_size.width),
                    )
                } else {
                    size(
                        image_size.width * (bounds.size.height / image_size.height),
                        bounds.size.height,
                    )
                };

                Bounds {
                    origin: point(
                        bounds.origin.x + (bounds.size.width - new_size.width) / 2.0,
                        bounds.origin.y + (bounds.size.height - new_size.height) / 2.0,
                    ),
                    size: new_size,
                }
            }
            ObjectFit::None => Bounds {
                origin: bounds.origin,
                size: image_size,
            },
        }
    }
}

impl Img {
    /// Set the image to be displayed in grayscale.
    pub fn grayscale(mut self, grayscale: bool) -> Self {
        self.grayscale = grayscale;
        self
    }
    /// Set the object fit for the image.
    pub fn object_fit(mut self, object_fit: ObjectFit) -> Self {
        self.object_fit = object_fit;
        self
    }
}

impl Element for Img {
    type BeforeLayout = ();
    type AfterLayout = Option<Hitbox>;

    fn before_layout(&mut self, cx: &mut ElementContext) -> (LayoutId, Self::BeforeLayout) {
        let layout_id = self.interactivity.before_layout(cx, |mut style, cx| {
            // TODO: Adjust this so that the vector data gets its 'natural' size here
            if let Some(data) = self.source.data(None, cx) {
                let image_size = data.size();
                match (style.size.width, style.size.height) {
                    (Length::Auto, Length::Auto) => {
                        style.size = Size {
                            width: Length::Definite(DefiniteLength::Absolute(
                                AbsoluteLength::Pixels(px(image_size.width.0 as f32)),
                            )),
                            height: Length::Definite(DefiniteLength::Absolute(
                                AbsoluteLength::Pixels(px(image_size.height.0 as f32)),
                            )),
                        }
                    }
                    _ => {}
                }
            }

            cx.request_layout(&style, [])
        });
        (layout_id, ())
    }

    fn after_layout(
        &mut self,
        bounds: Bounds<Pixels>,
        _before_layout: &mut Self::BeforeLayout,
        cx: &mut ElementContext,
    ) -> Option<Hitbox> {
        self.interactivity
            .after_layout(bounds, bounds.size, cx, |_, _, hitbox, _| hitbox)
    }

    fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        _: &mut Self::BeforeLayout,
        hitbox: &mut Self::AfterLayout,
        cx: &mut ElementContext,
    ) {
        let source = self.source.clone();
        self.interactivity
            .paint(bounds, hitbox.as_ref(), cx, |style, cx| {
                let corner_radii = style.corner_radii.to_pixels(bounds.size, cx.rem_size());

                if let Some(data) = source.data(Some(bounds), cx) {
                    cx.paint_image(bounds, corner_radii, data.clone(), self.grayscale)
                        .log_err();
                }

                match source {
                    #[cfg(target_os = "macos")]
                    ImageSource::Surface(surface) => {
                        let size = size(surface.width().into(), surface.height().into());
                        let new_bounds = self.object_fit.get_bounds(bounds, size);
                        // TODO: Add support for corner_radii and grayscale.
                        cx.paint_surface(new_bounds, surface);
                    }
                    _ => {}
                }
            })
    }
}

impl IntoElement for Img {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for Img {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.interactivity.base_style
    }
}

impl InteractiveElement for Img {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl ImageSource {
    fn data(
        &self,
        bounds: Option<Bounds<Pixels>>,
        cx: &mut ElementContext,
    ) -> Option<Arc<ImageData>> {
        match self {
            ImageSource::Uri(_) | ImageSource::File(_) => {
                let uri_or_path: UriOrPath = match self {
                    ImageSource::Uri(uri) => uri.clone().into(),
                    ImageSource::File(path) => path.clone().into(),
                    _ => unreachable!(),
                };

                let asset = cx.use_asset::<RasterOrVector>(&uri_or_path)?.log_err()?;

                match asset {
                    RasterOrVector::Raster(data) => Some(data),
                    RasterOrVector::Vector { data, id } => {
                        let bounds = bounds?;

                        let scaled = bounds.scale(cx.scale_factor());
                        let key = {
                            let size = scaled.size.map(|x| x.into());
                            VectorKey { data, id, size }
                        };

                        cx.use_asset::<Vector>(&key)
                    }
                }
            }

            ImageSource::Data(data) => Some(data.to_owned()),
            #[cfg(target_os = "macos")]
            ImageSource::Surface(_) => None,
        }
    }
}

#[derive(Clone)]
enum RasterOrVector {
    Raster(Arc<ImageData>),
    Vector {
        data: Arc<resvg::usvg::Tree>,
        id: u64,
    },
}

impl Asset for RasterOrVector {
    type Source = UriOrPath;
    type Output = Result<Self, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut AppContext,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let mut asset_cache = cx.asset_cache();

        async move {
            if let Some(asset) = asset_cache.get::<Self>(&source) {
                return asset.clone();
            }

            let bytes = match source.clone() {
                UriOrPath::Path(uri) => fs::read(uri.as_ref())?,
                UriOrPath::Uri(uri) => {
                    let mut response = client.get(uri.as_ref(), ().into(), true).await?;
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

            let data = if let Ok(format) = image::guess_format(&bytes) {
                let data = image::load_from_memory_with_format(&bytes, format)?.into_rgba8();
                Self::Raster(Arc::new(ImageData::new(data)))
            } else {
                let data = resvg::usvg::Tree::from_data(
                    &bytes,
                    &resvg::usvg::Options::default(),
                    svg_fontdb(),
                )?;

                let id = hash(&source);

                Self::Vector {
                    data: Arc::new(data),
                    id,
                }
            };

            asset_cache.insert::<Self>(source, Ok(data.clone()));

            Ok(data)
        }
    }

    fn remove_from_cache(source: &Self::Source, cx: &mut AppContext) -> Option<Self::Output> {
        cx.asset_cache().remove::<Self>(source)
    }
}

#[derive(Clone)]
struct VectorKey {
    data: Arc<resvg::usvg::Tree>,
    id: u64,
    size: Size<DevicePixels>,
}

impl Hash for VectorKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.size.hash(state);
    }
}

struct Vector;

impl Asset for Vector {
    type Source = VectorKey;
    type Output = Arc<ImageData>;

    fn load(
        source: Self::Source,
        cx: &mut AppContext,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let mut asset_cache = cx.asset_cache();

        async move {
            if let Some(image_data) = asset_cache.get::<Self>(&source) {
                return image_data.clone();
            };

            let mut pixmap = resvg::tiny_skia::Pixmap::new(
                source.size.width.0 as u32,
                source.size.height.0 as u32,
            )
            .unwrap();
            let ratio = source.size.width.0 as f32 / source.data.size().width();
            resvg::render(
                &source.data,
                resvg::tiny_skia::Transform::from_scale(ratio, ratio),
                &mut pixmap.as_mut(),
            );
            let png = pixmap.encode_png().unwrap();
            let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
            let image_data = Arc::new(ImageData::new(image.into_rgba8()));
            asset_cache.insert::<Self>(source.clone(), image_data.clone());

            image_data
        }
    }

    fn remove_from_cache(source: &Self::Source, cx: &mut AppContext) -> Option<Self::Output> {
        cx.asset_cache().remove::<Self>(source)
    }
}

/// An error that can occur when interacting with the image cache.
#[derive(Debug, Error, Clone)]
pub enum ImageCacheError {
    /// An error that occurred while fetching an image from a remote source.
    #[error("http error: {0}")]
    Client(#[from] http::Error),
    /// An error that occurred while reading the image from disk.
    #[error("IO error: {0}")]
    Io(Arc<std::io::Error>),
    /// An error that occurred while processing an image.
    #[error("unexpected http status: {status}, body: {body}")]
    BadStatus {
        /// The HTTP status code.
        status: http::StatusCode,
        /// The HTTP response body.
        body: String,
    },
    /// An error that occurred while processing an image.
    #[error("image error: {0}")]
    Image(Arc<ImageError>),
    /// An error that occurred while processing an SVG.
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
