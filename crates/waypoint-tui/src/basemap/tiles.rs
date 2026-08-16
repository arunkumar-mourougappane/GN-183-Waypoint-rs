//! Slippy-tile arithmetic and the on-disk tile store.

use std::path::{Path, PathBuf};

/// Web Mercator cannot represent the poles; this is the conventional cut-off.
const MAX_LATITUDE: f64 = 85.051_129;
/// OpenMapTiles data stops here — deeper zooms are rendered by over-zooming the
/// z14 tile rather than by fetching one that does not exist.
pub const MAX_ZOOM: u8 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    /// West/south/east/north bounds of this tile, in degrees.
    pub fn bounds(self) -> (f64, f64, f64, f64) {
        let n = tiles_across(self.zoom) as f64;
        let west = self.x as f64 / n * 360.0 - 180.0;
        let east = (self.x + 1) as f64 / n * 360.0 - 180.0;
        (west, tile_lat(self.y + 1, n), east, tile_lat(self.y, n))
    }
}

fn tiles_across(zoom: u8) -> u32 {
    1u32 << zoom
}

fn tile_lat(y: u32, n: f64) -> f64 {
    let t = std::f64::consts::PI * (1.0 - 2.0 * y as f64 / n);
    t.sinh().atan().to_degrees()
}

/// Tile containing this position.
pub fn tile_at(lat: f64, lon: f64, zoom: u8) -> TileId {
    let n = tiles_across(zoom) as f64;
    let lat = lat.clamp(-MAX_LATITUDE, MAX_LATITUDE);
    let x = ((lon + 180.0) / 360.0 * n).floor().clamp(0.0, n - 1.0) as u32;
    let y = ((1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n)
        .floor()
        .clamp(0.0, n - 1.0) as u32;
    TileId { zoom, x, y }
}

/// Deepest zoom whose tiles still cover the box in at most `budget` tiles.
///
/// Detail is wasted if the whole view is one tile, and fetching a hundred of
/// them to fill a terminal panel is worse — this picks the most detail that
/// stays within the budget.
pub fn zoom_for_bounds(west: f64, south: f64, east: f64, north: f64, budget: u32) -> u8 {
    for zoom in (0..=MAX_ZOOM).rev() {
        let top_left = tile_at(north, west, zoom);
        let bottom_right = tile_at(south, east, zoom);
        let wide = bottom_right.x.abs_diff(top_left.x) + 1;
        let tall = bottom_right.y.abs_diff(top_left.y) + 1;
        if wide * tall <= budget {
            return zoom;
        }
    }
    0
}

/// Every tile touching the box, at the given zoom.
pub fn tiles_covering(west: f64, south: f64, east: f64, north: f64, zoom: u8) -> Vec<TileId> {
    let top_left = tile_at(north, west, zoom);
    let bottom_right = tile_at(south, east, zoom);

    let (x0, x1) = (
        top_left.x.min(bottom_right.x),
        top_left.x.max(bottom_right.x),
    );
    let (y0, y1) = (
        top_left.y.min(bottom_right.y),
        top_left.y.max(bottom_right.y),
    );

    let mut out = Vec::new();
    for x in x0..=x1 {
        for y in y0..=y1 {
            out.push(TileId { zoom, x, y });
        }
    }
    out
}

/// Where a tile lives on disk.
pub fn cache_path(root: &Path, tile: TileId) -> PathBuf {
    root.join(tile.zoom.to_string())
        .join(tile.x.to_string())
        .join(format!("{}.pbf", tile.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checked against the reference slippy-map formula: Greenwich at z14.
    #[test]
    fn tile_at_matches_the_reference_projection() {
        let tile = tile_at(51.4779, 0.0042, 14);
        assert_eq!((tile.x, tile.y), (8192, 5450));

        // The prime meridian is the boundary between two tile columns.
        assert_eq!(tile_at(51.4779, -0.0042, 14).x, 8191);
    }

    #[test]
    fn tile_bounds_contain_their_own_centre() {
        let tile = tile_at(51.4779, 0.0042, 14);
        let (west, south, east, north) = tile.bounds();
        assert!(west < 0.0042 && 0.0042 < east, "{west}..{east}");
        assert!(south < 51.4779 && 51.4779 < north, "{south}..{north}");
    }

    #[test]
    fn zoom_backs_off_until_the_box_fits_the_budget() {
        // A city-block-sized box gets the deepest zoom available.
        assert_eq!(zoom_for_bounds(0.000, 51.477, 0.002, 51.479, 4), MAX_ZOOM);

        // A whole-country box has to zoom out to stay within the same budget.
        let country = zoom_for_bounds(-8.0, 50.0, 2.0, 58.0, 4);
        assert!(country < 8, "expected a coarse zoom, got {country}");
    }

    #[test]
    fn coverage_includes_every_tile_the_box_touches() {
        let tiles = tiles_covering(-0.01, 51.47, 0.01, 51.49, 14);
        assert!(tiles.len() >= 4, "a box across the meridian spans columns");
        assert!(tiles.iter().any(|t| t.x == 8191));
        assert!(tiles.iter().any(|t| t.x == 8192));
    }
}
