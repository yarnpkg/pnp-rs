use pnp_rs::{pnp_resolve, ResolutionConfig, Resolution};
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args();

    // Skip the program name
    args.next();

    let specifier = args.next()
        .expect("A specifier must be provided");

    let parent = args.next()
        .map(PathBuf::from)
        .expect("A parent url must be provided");

    println!("specifier = {}", specifier);
    println!("parent    = {:?}", parent);

    let resolution = pnp_resolve(&specifier, &parent, &ResolutionConfig {
        ..Default::default()
    });

    match resolution {
        Ok(res) => {
            match res {
                Resolution::Path(p) => {
                    println!("result    = Path ({:?})", p);
                }
                Resolution::Specifier(specifier) => {
                    println!("result    = Specifier ({})", specifier);
                }
            }
        }
        Err(err) => {
            println!("error     = {}", err);
        }
    }
}
