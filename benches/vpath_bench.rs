use criterion::{Criterion, criterion_group, criterion_main};
use pnp::fs::VPath;
use std::hint::black_box;
use std::path::Path;
use std::time::Duration;

fn bench_vpath_native(c: &mut Criterion) {
    let paths = vec![
        "/simple/path",
        "/usr/local/bin",
        "/home/user/project/src/main.rs",
        "/very/long/path/with/many/segments/that/could/be/expensive/to/parse",
    ];

    c.bench_function("vpath_native", |b| {
        b.iter(|| {
            for path in &paths {
                let _ = VPath::from(black_box(Path::new(path)));
            }
        })
    });
}

fn bench_vpath_virtual(c: &mut Criterion) {
    let paths = vec![
        "/node_modules/.yarn/__virtual__/abc123/0/node_modules/package",
        "/project/__virtual__/def456/2/node_modules/some-package/lib/index.js",
        "/deep/path/__virtual__/ghi789/1/node_modules/nested/package/src/file.ts",
        "/complex/__virtual__/jkl012/3/node_modules/very-long-package-name/dist/bundle.js",
    ];

    c.bench_function("vpath_virtual", |b| {
        b.iter(|| {
            for path in &paths {
                let _ = VPath::from(black_box(Path::new(path)));
            }
        })
    });
}

fn bench_vpath_zip(c: &mut Criterion) {
    let paths = vec![
        "/cache/package.zip/lib/index.js",
        "/node_modules/package/archive.zip/src/main.ts",
        "/project/deps/bundle.zip/dist/app.js",
        "/deep/path/to/package.zip/nested/file.json",
    ];

    c.bench_function("vpath_zip", |b| {
        b.iter(|| {
            for path in &paths {
                let _ = VPath::from(black_box(Path::new(path)));
            }
        })
    });
}

fn bench_vpath_virtual_zip(c: &mut Criterion) {
    let paths = vec![
        "/node_modules/.yarn/__virtual__/abc123/0/node_modules/package/archive.zip/lib/index.js",
        "/project/__virtual__/def456/2/node_modules/some-package/bundle.zip/dist/app.js",
        "/deep/path/__virtual__/ghi789/1/node_modules/nested/package.zip/src/file.ts",
    ];

    c.bench_function("vpath_virtual_zip", |b| {
        b.iter(|| {
            for path in &paths {
                let _ = VPath::from(black_box(Path::new(path)));
            }
        })
    });
}

fn bench_vpath_edge_cases(c: &mut Criterion) {
    let paths = vec![
        "",
        "/",
        "//",
        "/single",
        "/path/with/./dots/../and/stuff",
        "/path/with/spaces in names",
        "/path/with/unicode/🦀/names",
        "/malformed/__virtual__/incomplete",
        "/fake/__virtual__/abc123/not_a_number/path",
    ];

    c.bench_function("vpath_edge_cases", |b| {
        b.iter(|| {
            for path in &paths {
                let _ = VPath::from(black_box(Path::new(path)));
            }
        })
    });
}

fn bench_vpath_mixed_workload(c: &mut Criterion) {
    let paths = vec![
        "/simple/native/path",
        "/node_modules/.yarn/__virtual__/abc123/0/node_modules/package",
        "/cache/package.zip/lib/index.js",
        "/project/__virtual__/def456/2/node_modules/some-package/bundle.zip/dist/app.js",
        "/usr/local/bin/executable",
        "/deep/path/__virtual__/ghi789/1/node_modules/nested/package/src/file.ts",
        "/project/deps/bundle.zip/nested/file.json",
        "/home/user/project/src/main.rs",
    ];

    c.bench_function("vpath_mixed_workload", |b| {
        b.iter(|| {
            for path in &paths {
                let _ = VPath::from(black_box(Path::new(path)));
            }
        })
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(1000).measurement_time(Duration::from_secs(10));
    targets = bench_vpath_native, bench_vpath_virtual, bench_vpath_zip, bench_vpath_virtual_zip, bench_vpath_edge_cases, bench_vpath_mixed_workload
}

criterion_main!(benches);
