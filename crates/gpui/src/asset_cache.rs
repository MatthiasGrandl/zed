use crate::{AppContext, SharedUri, Task};
use collections::HashMap;
use futures::future::Shared;
use parking_lot::Mutex;
use std::any::TypeId;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::{any::Any, path::PathBuf};
use util::http::HttpClient;

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

/// A task for fetching an asset.
pub type AssetFetchTask<A: Asset> = Shared<Task<Result<A::Output, A::Error>>>;

/// A trait for asynchronous asset loading.
pub trait Asset {
    /// The source of the asset.
    type Source: Clone + Hash;
    /// The loaded asset.
    type Output: Clone;
    /// The error type that can occur during loading.
    type Error: Clone;
    /// Load the asset asynchronously, might make use of cache.
    fn load(source: &Self::Source, cx: &mut AppContext) -> AssetFetchTask<Self>;
}

/// A cache for assets.
pub struct AssetCache {
    client: Arc<dyn HttpClient>,
    assets: Arc<Mutex<HashMap<(TypeId, u64), Shared<Task<Box<dyn Any>>>>>>,
}

impl AssetCache {
    pub(crate) fn new(client: Arc<dyn HttpClient>) -> Self {
        Self {
            client,
            assets: Default::default(),
        }
    }

    /// Get the asset from the cache, if it exists.
    pub fn get<A: Asset>(&self, source: &A::Source) -> Option<&AssetFetchTask<A>> {
        let mut hasher = DefaultHasher::new();
        source.hash(&mut hasher);
        let hash = hasher.finish();
        self.assets.lock().get(&(TypeId::of::<A>(), hash))
    }

    /// Insert the asset into the cache.
    pub fn insert<A: Asset>(&mut self, source: A::Source, task: AssetFetchTask<A>) {
        let mut hasher = DefaultHasher::new();
        source.hash(&mut hasher);
        let hash = hasher.finish();
        self.assets.lock().insert((TypeId::of::<A>(), hash), task);
    }

    /// Get the HTTP client used by this asset cache.
    pub fn client(&self) -> &Arc<dyn HttpClient> {
        &self.client
    }
}
