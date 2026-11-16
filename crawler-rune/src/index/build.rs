fn main() {
    let source = std::path::Path::new(file!()).with_file_name("performance.c");
    println!("cargo:rerun-if-changed={}", source.display());
    cc::Build::new()
        .file(source)
        .compile("crawler_search_performance");
}
