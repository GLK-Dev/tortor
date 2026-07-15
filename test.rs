fn main() {
    std::fs::create_dir_all("test_dir").unwrap();
    let meta = std::fs::metadata("test_dir").unwrap();
    println!("Dir length: {}", meta.len());
}
