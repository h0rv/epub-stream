#![cfg(feature = "std")]

use std::fs::File;
use std::path::PathBuf;

use epub_stream::metadata::{parse_container_xml, parse_opf};
use epub_stream::zip::StreamingZip;

fn read_entry_buffered(zip: &mut StreamingZip<File>, path: &str) -> Option<Vec<u8>> {
    let entry = zip.get_entry(path)?.clone();
    let mut out = vec![0u8; entry.uncompressed_size as usize];
    let n = zip.read_file(&entry, &mut out).ok()?;
    out.truncate(n);
    Some(out)
}

fn read_entry_streamed(zip: &mut StreamingZip<File>, path: &str) -> Option<Vec<u8>> {
    let entry = zip.get_entry(path)?.clone();
    let mut out = Vec::with_capacity(0);
    zip.read_file_to_writer(&entry, &mut out).ok()?;
    Some(out)
}

#[test]
#[ignore = "requires local Gutenberg dataset at tests/datasets/wild/gutenberg"]
fn buffered_and_streamed_entry_reads_match_for_problem_files() {
    let files = ["pg74.epub", "pg98.epub"];

    for name in files {
        let path = PathBuf::from("tests/datasets/wild/gutenberg").join(name);
        if !path.exists() {
            continue;
        }

        let file = File::open(&path).expect("open epub");
        let mut zip = StreamingZip::new(file).expect("parse zip");

        let container = read_entry_buffered(&mut zip, "META-INF/container.xml")
            .expect("read container via buffered path");
        let opf_path = parse_container_xml(&container).expect("parse container xml");

        let opf_buffered =
            read_entry_buffered(&mut zip, &opf_path).expect("read opf via buffered path");
        let opf_streamed =
            read_entry_streamed(&mut zip, &opf_path).expect("read opf via streamed path");

        assert_eq!(
            opf_buffered, opf_streamed,
            "buffered and streamed OPF bytes diverged for {}",
            name
        );
        assert!(
            parse_opf(&opf_buffered).is_ok(),
            "buffered OPF parse failed for {}",
            name
        );
        assert!(
            parse_opf(&opf_streamed).is_ok(),
            "streamed OPF parse failed for {}",
            name
        );
    }
}
