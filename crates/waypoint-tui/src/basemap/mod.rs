//! A basemap for the track canvas: roads, water and green space drawn beneath
//! the track so a position has somewhere to be.
//!
//! Tiles come from the network when it is reachable and from a cache on disk
//! when it is not, so the same session works at a desk and in a field with no
//! signal. Everything fetched is cached, which is also what makes the offline
//! path possible: you cannot use a tile you have never once been able to fetch.

mod decode;
mod tiles;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

pub use decode::{Feature, Shape};
pub use tiles::TileId;

/// Resolves to the current planet build. The tile URL embeds a dated snapshot
/// that is retired periodically, so it is looked up rather than hardcoded — a
/// pinned URL is a basemap that stops working on someone else's schedule.
const TILEJSON_URL: &str = "https://tiles.openfreemap.org/planet";
/// Attribution required by the data's licence, shown on the track panel.
pub const ATTRIBUTION: &str = "© OpenMapTiles © OpenStreetMap";

/// Tiles to fetch for one view. A terminal panel cannot show more detail than
/// this, and it keeps the demand on a free tile server modest.
const TILE_BUDGET: u32 = 4;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// No basemap wanted.
    Disabled,
    /// Nothing fetched yet.
    Idle,
    Loading,
    /// Drawing tiles, at least one of which came from the network.
    Online,
    /// Drawing tiles, all of them from the cache.
    Offline,
    /// Nothing to draw and nowhere to get it.
    Unavailable,
}

impl Availability {
    pub fn label(self) -> &'static str {
        match self {
            Availability::Disabled => "map off",
            Availability::Idle => "map idle",
            Availability::Loading => "map loading",
            Availability::Online => "map online",
            Availability::Offline => "map cached",
            Availability::Unavailable => "map unavailable",
        }
    }
}

/// Shared basemap state. Cheap to clone; the work happens on a background task.
#[derive(Clone)]
pub struct Basemap {
    inner: Arc<Inner>,
}

struct Inner {
    features: RwLock<Vec<Feature>>,
    requested: Mutex<HashSet<TileId>>,
    /// The view the current features were fetched for, so a small drift does
    /// not trigger a refetch.
    covering: Mutex<Option<(u8, Vec<TileId>)>>,
    endpoint: Mutex<Option<String>>,
    cache_root: Option<PathBuf>,
    availability: Mutex<Availability>,
    used_network: AtomicBool,
    enabled: bool,
}

impl Basemap {
    pub fn new(enabled: bool) -> Self {
        // Platform-appropriate: ~/Library/Caches on macOS, %LOCALAPPDATA% on
        // Windows, $XDG_CACHE_HOME on Linux.
        let cache_root = directories::ProjectDirs::from("", "", "waypoint")
            .map(|dirs| dirs.cache_dir().join("tiles"));

        Self {
            inner: Arc::new(Inner {
                features: RwLock::new(Vec::new()),
                requested: Mutex::new(HashSet::new()),
                covering: Mutex::new(None),
                endpoint: Mutex::new(None),
                cache_root,
                availability: Mutex::new(if enabled {
                    Availability::Idle
                } else {
                    Availability::Disabled
                }),
                used_network: AtomicBool::new(false),
                enabled,
            }),
        }
    }

    pub fn cache_dir(&self) -> Option<&PathBuf> {
        self.inner.cache_root.as_ref()
    }

    pub fn availability(&self) -> Availability {
        *self
            .inner
            .availability
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Shapes to draw, back to front.
    pub fn features(&self) -> Vec<Feature> {
        self.inner
            .features
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Ask for coverage of a bounding box. Returns immediately; tiles arrive on
    /// a background task and appear at the next redraw.
    pub fn request(&self, west: f64, south: f64, east: f64, north: f64) {
        if !self.inner.enabled {
            return;
        }

        let zoom = tiles::zoom_for_bounds(west, south, east, north, TILE_BUDGET);
        let wanted = tiles::tiles_covering(west, south, east, north, zoom);

        {
            // Already covering this view, so there is nothing to do.
            let covering = self
                .inner
                .covering
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if covering
                .as_ref()
                .is_some_and(|(z, t)| *z == zoom && *t == wanted)
            {
                return;
            }
        }

        let fresh: Vec<TileId> = {
            let mut requested = self
                .inner
                .requested
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            wanted
                .iter()
                .copied()
                .filter(|tile| requested.insert(*tile))
                .collect()
        };

        *self
            .inner
            .covering
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some((zoom, wanted));
        if fresh.is_empty() {
            return;
        }

        let this = self.clone();
        tokio::spawn(async move { this.load(fresh).await });
    }

    async fn load(self, wanted: Vec<TileId>) {
        self.set_availability(Availability::Loading);

        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // Tile servers identify clients by user agent, and a free one is
            // entitled to know who is asking.
            .user_agent(concat!("waypoint-tui/", env!("CARGO_PKG_VERSION")))
            .build()
            .ok();

        let mut decoded = Vec::new();
        let mut any = false;

        for tile in wanted {
            let Some(bytes) = self.tile_bytes(client.as_ref(), tile).await else {
                continue;
            };
            match decode::decode_tile(tile, bytes) {
                Ok(features) => {
                    any = true;
                    decoded.extend(features);
                }
                Err(err) => tracing::warn!(?tile, %err, "vector tile would not decode"),
            }
        }

        let decoded_len = decoded.len();
        if any {
            // Back to front, so roads land on top of the water they cross.
            decoded.sort_by_key(|f| f.shape);
            let mut features = self
                .inner
                .features
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *features = decoded;
        }

        tracing::debug!(features = decoded_len, "basemap tiles loaded");
        self.set_availability(
            match (any, self.inner.used_network.load(Ordering::Relaxed)) {
                (false, _) => Availability::Unavailable,
                (true, true) => Availability::Online,
                (true, false) => Availability::Offline,
            },
        );
    }

    /// A tile's bytes: from the cache if it is there, from the network if not.
    ///
    /// Cache-first rather than network-first. "Prefer online" is about where a
    /// tile is *obtained*; re-downloading one already on disk would only cost
    /// the tile server bandwidth and the user their time.
    async fn tile_bytes(&self, client: Option<&reqwest::Client>, tile: TileId) -> Option<Vec<u8>> {
        if let Some(root) = &self.inner.cache_root {
            let path = tiles::cache_path(root, tile);
            if let Ok(bytes) = tokio::fs::read(&path).await
                && !bytes.is_empty()
            {
                tracing::debug!(?tile, "tile from cache");
                return Some(bytes);
            }
        }

        let client = client?;
        let url = self.tile_url(client, tile).await?;
        let response = client.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            tracing::warn!(?tile, status = %response.status(), "tile request refused");
            return None;
        }
        let bytes = response.bytes().await.ok()?.to_vec();
        if bytes.is_empty() {
            return None;
        }

        tracing::debug!(?tile, bytes = bytes.len(), "tile from network");
        self.inner.used_network.store(true, Ordering::Relaxed);
        if let Some(root) = &self.inner.cache_root {
            let path = tiles::cache_path(root, tile);
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            // A cache write failing is not worth failing the draw over.
            if let Err(err) = tokio::fs::write(&path, &bytes).await {
                tracing::warn!(%err, "could not cache tile");
            }
        }
        Some(bytes)
    }

    /// Resolve the tile URL template once per session, then fill in the tile.
    async fn tile_url(&self, client: &reqwest::Client, tile: TileId) -> Option<String> {
        {
            let endpoint = self
                .inner
                .endpoint
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(template) = endpoint.as_ref() {
                return Some(fill_template(template, tile));
            }
        }

        let body = client
            .get(TILEJSON_URL)
            .send()
            .await
            .ok()?
            .text()
            .await
            .ok()?;
        let template = first_tile_template(&body)?;
        *self
            .inner
            .endpoint
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(template.clone());
        Some(fill_template(&template, tile))
    }

    fn set_availability(&self, availability: Availability) {
        *self
            .inner
            .availability
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = availability;
    }
}

fn fill_template(template: &str, tile: TileId) -> String {
    template
        .replace("{z}", &tile.zoom.to_string())
        .replace("{x}", &tile.x.to_string())
        .replace("{y}", &tile.y.to_string())
}

/// Pull the first entry of TileJSON's `tiles` array.
///
/// Hand-parsed rather than pulling in a JSON dependency for one field: the
/// document is a fixed shape from a known server, and the array is the only
/// thing wanted from it.
fn first_tile_template(body: &str) -> Option<String> {
    let after = body.split("\"tiles\"").nth(1)?;
    let open = after.find('[')?;
    let close = after[open..].find(']')? + open;
    let first = after[open + 1..close].split(',').next()?;
    let url = first.trim().trim_matches('"');
    url.starts_with("http").then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_template_is_read_out_of_tilejson() {
        let body = r#"{"tilejson":"3.0.0","tiles":["https://tiles.example.org/planet/20260802/{z}/{x}/{y}.pbf"],"maxzoom":14}"#;
        let template = first_tile_template(body).expect("a tiles array");
        assert_eq!(
            fill_template(
                &template,
                TileId {
                    zoom: 14,
                    x: 8192,
                    y: 5450
                }
            ),
            "https://tiles.example.org/planet/20260802/14/8192/5450.pbf"
        );
    }

    /// A pinned snapshot goes stale, so a malformed or empty document must not
    /// be mistaken for a usable endpoint.
    #[test]
    fn a_document_without_tiles_yields_nothing() {
        assert_eq!(first_tile_template("{}"), None);
        assert_eq!(first_tile_template(r#"{"tiles":[]}"#), None);
        assert_eq!(first_tile_template(r#"{"tiles":["ftp://nope"]}"#), None);
    }

    #[test]
    fn a_disabled_basemap_never_requests_anything() {
        let map = Basemap::new(false);
        map.request(-0.01, 51.47, 0.01, 51.49);
        assert_eq!(map.availability(), Availability::Disabled);
        assert!(map.features().is_empty());
    }

    #[test]
    fn the_cache_lives_under_the_platform_cache_directory() {
        let map = Basemap::new(true);
        let dir = map.cache_dir().expect("a platform cache directory");
        assert!(dir.ends_with("tiles"), "unexpected cache path {dir:?}");
    }
}
