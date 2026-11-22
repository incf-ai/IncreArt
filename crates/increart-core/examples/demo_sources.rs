//! Downloads the public-domain source images of the demo library, with a
//! credits file saying where each came from and why it may be redistributed.
//!
//!     cargo run -p increart-core --example demo_sources [out_dir]     # default: tmp/demo-sources
//!
//! Every source is checked against its collection's own rights field (Met
//! `isPublicDomain`, Art Institute `is_public_domain`, the Library of
//! Congress item's rights statement); one that no longer passes stops it.
//! NASA imagery is not subject to US copyright (NASA media guidelines).
#![allow(clippy::disallowed_methods)] // fixtures on disk: no overlay here

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// (file stem, collection, id)
const SOURCES: [(&str, &str, &str); 9] = [
    ("great-wave", "met", "45434"),
    ("wheat-field", "met", "436535"),
    ("grande-jatte", "aic", "27992"),
    ("moonwalk", "nasa", "as11-40-5903"),
    ("earthrise", "nasa", "as08-14-2383"),
    ("zion-poster", "loc", "2007676134"),
    ("grand-canyon-poster", "loc", "2007676131"),
    ("liberty-photochrom", "loc", "2008679689"),
    ("sydney-harbour", "loc", "2017657736"),
];

#[derive(Serialize)]
struct Credit {
    file: String,
    title: String,
    creator: String,
    date: String,
    collection: &'static str,
    page: String,
    rights: &'static str,
    image: String,
}

fn get(agent: &ureq::Agent, url: &str) -> Vec<u8> {
    let mut resp = agent.get(url).header("User-Agent", "increart-demo-sources/1.0").call().expect(url);
    resp.body_mut().with_config().limit(u64::MAX).read_to_vec().expect(url)
}

fn json(agent: &ureq::Agent, url: &str) -> Value {
    serde_json::from_slice(&get(agent, url)).expect(url)
}

/// A string field, "" when missing or null.
fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn credit(agent: &ureq::Agent, kind: &str, id: &str) -> Credit {
    match kind {
        "met" => {
            let d = json(agent, &format!("https://collectionapi.metmuseum.org/public/collection/v1/objects/{id}"));
            assert!(d["isPublicDomain"].as_bool() == Some(true), "met {id} is not public domain");
            Credit {
                file: String::new(),
                title: s(&d["title"]),
                creator: s(&d["artistDisplayName"]),
                date: s(&d["objectDate"]),
                collection: "The Metropolitan Museum of Art",
                page: s(&d["objectURL"]),
                rights: "Public domain (Met Open Access, CC0)",
                image: s(&d["primaryImage"]),
            }
        }
        "aic" => {
            let fields = "id,title,artist_title,date_display,is_public_domain,image_id";
            let d = json(agent, &format!("https://api.artic.edu/api/v1/artworks/{id}?fields={fields}"));
            let a = &d["data"];
            assert!(a["is_public_domain"].as_bool() == Some(true), "aic {id} is not public domain");
            Credit {
                file: String::new(),
                title: s(&a["title"]),
                creator: s(&a["artist_title"]),
                date: s(&a["date_display"]),
                collection: "Art Institute of Chicago",
                page: format!("https://www.artic.edu/artworks/{id}"),
                rights: "Public domain (Art Institute of Chicago, CC0)",
                image: format!("{}/{}/full/1686,/0/default.jpg", s(&d["config"]["iiif_url"]), s(&a["image_id"])),
            }
        }
        "nasa" => {
            let d = json(agent, &format!("https://images-api.nasa.gov/search?nasa_id={id}"));
            let meta = &d["collection"]["items"][0]["data"][0];
            Credit {
                file: String::new(),
                title: s(&meta["title"]),
                creator: "NASA".into(),
                date: s(&meta["date_created"]).chars().take(10).collect(),
                collection: "NASA Image and Video Library",
                page: format!("https://images.nasa.gov/details/{id}"),
                rights: "NASA imagery, not subject to US copyright",
                image: format!("https://images-assets.nasa.gov/image/{id}/{id}~medium.jpg"),
            }
        }
        "loc" => {
            let d = json(agent, &format!("https://www.loc.gov/pictures/item/{id}/?fo=json"));
            let item = &d["item"];
            let rights = s(&item["rights_information"]);
            let short: String = rights.chars().take(120).collect();
            assert!(rights.to_lowercase().contains("no known restrictions"), "loc {id} rights: {short}");
            Credit {
                file: String::new(),
                title: s(&item["title"]),
                creator: s(&item["creator"]),
                date: s(&item["date"]),
                collection: "Library of Congress Prints and Photographs",
                page: format!("https://www.loc.gov/pictures/item/{id}/"),
                rights: "No known restrictions on publication (Library of Congress)",
                image: s(&d["resources"][0]["large"]),
            }
        }
        _ => unreachable!("{kind}"),
    }
}

fn main() {
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/demo-sources");
    let out = std::env::args().nth(1).map_or(default, PathBuf::from);
    std::fs::create_dir_all(&out).unwrap();
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60))).build().into();
    let mut credits = vec![];
    for (stem, kind, id) in SOURCES {
        let info = Credit { file: format!("{stem}.jpg"), ..credit(&agent, kind, id) };
        let path = out.join(&info.file);
        if !path.exists() {
            let part = out.join(format!("{}.part", info.file));
            std::fs::write(&part, get(&agent, &info.image)).unwrap();
            std::fs::rename(&part, &path).unwrap();
        }
        println!("{stem:22} {}", info.rights);
        credits.push(info);
    }
    let text = serde_json::to_string_pretty(&credits).unwrap() + "\n";
    std::fs::write(out.join("credits.json"), text).unwrap();
}
