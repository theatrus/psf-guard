use std::path::PathBuf;

use seiza::catalog::{StarCatalog, TileCatalog, TileSetBuilder};

fn main() {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let source = args
        .next()
        .expect("usage: generate_astrometry_e2e_stars <source> <output>");
    let output = args
        .next()
        .expect("usage: generate_astrometry_e2e_stars <source> <output>");
    let catalog = TileCatalog::open(&source).expect("open source star catalog");
    let stars = catalog.cone_search(130.107013851174, 19.6601508517091, 4.0, 5_000);
    assert!(stars.len() >= 100, "fixture needs enough stars to solve");

    let mut builder = TileSetBuilder::new(45, catalog.epoch(), catalog.attribution());
    for star in stars {
        builder.add(star.ra, star.dec, star.mag);
    }
    builder.write_to(&output).expect("write fixture catalog");
}
