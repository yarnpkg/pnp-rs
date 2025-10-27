use pnp::{Resolution, ResolutionConfig};
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args();

    // Skip the program name
    args.next();

    let specifier = args.next().expect("A specifier must be provided");

    let parent = args.next().map(PathBuf::from).expect("A parent url must be provided");

    println!("specifier = {specifier}");
    println!("parent    = {parent:?}");

    let resolution = pnp::resolve_to_unqualified(&specifier, &parent, &ResolutionConfig::default());

    match resolution {
        Ok(res) => match res {
            Resolution::Resolved(p, subpath) => {
                println!("result    = Package ({p:?}, {subpath:?})");
            }
            Resolution::Skipped => {
                println!("result    = Skipped");
            }
        },
        Err(err) => {
            println!("{err}");
        }
    }
}
