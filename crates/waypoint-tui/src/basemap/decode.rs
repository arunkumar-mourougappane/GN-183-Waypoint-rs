//! Decoding Mapbox Vector Tiles into polylines the canvas can stroke.
//!
//! Only the layers worth a terminal's resolution are kept. A braille canvas
//! offers a few thousand dots, so buildings and footpaths would render as noise;
//! roads, water and green space are what actually read at this size.

use geo_types::Geometry;
use mvt_reader::Reader;
use mvt_reader::feature::Value;

use super::tiles::TileId;

/// What a shape is, which decides how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shape {
    // Ordered back to front: later variants are drawn over earlier ones.
    Landcover,
    Park,
    Water,
    Waterway,
    MinorRoad,
    MajorRoad,
    Motorway,
    Rail,
}

impl Shape {
    /// Whether this shape is worth drawing in a wide view. These are the ones
    /// that tell you where you are — a river, a motorway, a main road — as
    /// opposed to the detail that only reads close up.
    pub fn is_landmark(self) -> bool {
        matches!(
            self,
            Shape::Water | Shape::Waterway | Shape::Motorway | Shape::MajorRoad
        )
    }

    /// OpenMapTiles classifies roads in the `class` field; everything else is
    /// decided by which layer it came from.
    fn for_feature(layer: &str, class: Option<&str>) -> Option<Self> {
        match layer {
            "water" => Some(Shape::Water),
            "waterway" => Some(Shape::Waterway),
            "park" => Some(Shape::Park),
            "landcover" => Some(Shape::Landcover),
            "transportation" => match class? {
                "motorway" | "trunk" => Some(Shape::Motorway),
                "primary" | "secondary" | "tertiary" => Some(Shape::MajorRoad),
                "minor" | "service" | "street" => Some(Shape::MinorRoad),
                "rail" | "transit" => Some(Shape::Rail),
                // Paths and tracks are noise at this resolution.
                _ => None,
            },
            _ => None,
        }
    }
}

/// A shape in geographic coordinates, ready to project onto the canvas.
#[derive(Debug, Clone)]
pub struct Feature {
    pub shape: Shape,
    /// Longitude/latitude pairs, as a connected run.
    pub points: Vec<(f64, f64)>,
}

/// Decode one tile's worth of features.
///
/// Vector tile coordinates are tile-local integers spanning the layer's
/// `extent` — 4096 by convention, but declared per layer, so it is read rather
/// than assumed. Each point is scaled into the tile's geographic bounds, an
/// equirectangular approximation that within one z14 tile (about two kilometres)
/// stays far below a single braille dot.
pub fn decode_tile(tile: TileId, bytes: Vec<u8>) -> Result<Vec<Feature>, String> {
    let reader = Reader::new(bytes).map_err(|e| format!("{e:?}"))?;
    let layer_names = reader.get_layer_names().map_err(|e| format!("{e:?}"))?;
    let extents: Vec<f64> = reader
        .get_layer_metadata()
        .map(|layers| layers.iter().map(|l| l.extent.max(1) as f64).collect())
        .unwrap_or_default();
    let (west, south, east, north) = tile.bounds();

    let mut out = Vec::new();
    for (index, layer_name) in layer_names.iter().enumerate() {
        // Skip whole layers we never draw before doing any geometry work.
        if !matches!(
            layer_name.as_str(),
            "water" | "waterway" | "park" | "landcover" | "transportation"
        ) {
            continue;
        }

        let Ok(features) = reader.get_features(index) else {
            continue;
        };
        let extent = extents.get(index).copied().unwrap_or(4096.0);

        for feature in features {
            let class = feature
                .properties
                .as_ref()
                .and_then(|p| p.get("class"))
                .and_then(string_value);
            let Some(shape) = Shape::for_feature(layer_name, class) else {
                continue;
            };

            let mut runs = Vec::new();
            collect_runs(&feature.geometry, &mut runs);
            for run in runs {
                if run.len() < 2 {
                    continue;
                }
                out.push(Feature {
                    shape,
                    points: run
                        .into_iter()
                        .map(|(x, y)| {
                            let (u, v) = (x / extent, y / extent);
                            (west + u * (east - west), north + v * (south - north))
                        })
                        .collect(),
                });
            }
        }
    }
    Ok(out)
}

/// The `class` property is a string; anything else is not a road class.
fn string_value(value: &Value) -> Option<&str> {
    match value {
        Value::String(text) => Some(text.as_str()),
        _ => None,
    }
}

/// Flatten a geometry into connected runs of points. Polygons contribute their
/// outlines: a filled area is not something a braille grid can express, so the
/// boundary is what gets drawn.
fn collect_runs(geometry: &Geometry<f32>, out: &mut Vec<Vec<(f64, f64)>>) {
    match geometry {
        Geometry::LineString(line) => {
            out.push(line.0.iter().map(|c| (c.x as f64, c.y as f64)).collect());
        }
        Geometry::MultiLineString(lines) => {
            for line in &lines.0 {
                out.push(line.0.iter().map(|c| (c.x as f64, c.y as f64)).collect());
            }
        }
        Geometry::Polygon(polygon) => {
            out.push(
                polygon
                    .exterior()
                    .0
                    .iter()
                    .map(|c| (c.x as f64, c.y as f64))
                    .collect(),
            );
        }
        Geometry::MultiPolygon(polygons) => {
            for polygon in &polygons.0 {
                out.push(
                    polygon
                        .exterior()
                        .0
                        .iter()
                        .map(|c| (c.x as f64, c.y as f64))
                        .collect(),
                );
            }
        }
        Geometry::GeometryCollection(items) => {
            for item in items {
                collect_runs(item, out);
            }
        }
        // Points carry labels and markers, which this does not draw.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roads_are_classified_by_their_class_not_their_layer() {
        assert_eq!(
            Shape::for_feature("transportation", Some("motorway")),
            Some(Shape::Motorway)
        );
        assert_eq!(
            Shape::for_feature("transportation", Some("secondary")),
            Some(Shape::MajorRoad)
        );
        assert_eq!(Shape::for_feature("transportation", Some("path")), None);
        assert_eq!(Shape::for_feature("transportation", None), None);
    }

    #[test]
    fn only_layers_worth_the_resolution_are_kept() {
        assert_eq!(Shape::for_feature("water", None), Some(Shape::Water));
        assert_eq!(Shape::for_feature("park", None), Some(Shape::Park));
        // Buildings and labels would be noise on a braille grid.
        assert_eq!(Shape::for_feature("building", None), None);
        assert_eq!(Shape::for_feature("place", None), None);
    }

    #[test]
    fn wide_views_keep_only_the_shapes_that_orient_you() {
        assert!(Shape::Motorway.is_landmark());
        assert!(Shape::Water.is_landmark());
        // Side streets and ground cover are detail, not orientation.
        assert!(!Shape::MinorRoad.is_landmark());
        assert!(!Shape::Landcover.is_landmark());
        assert!(!Shape::Park.is_landmark());
    }

    /// Water must draw over landcover, and roads over both.
    #[test]
    fn shapes_sort_back_to_front() {
        let mut order = vec![Shape::Motorway, Shape::Landcover, Shape::Water];
        order.sort();
        assert_eq!(order, vec![Shape::Landcover, Shape::Water, Shape::Motorway]);
    }
}
