fn main() {
    let path = std::env::args().nth(1).expect("font");
    let data = std::fs::read(path).unwrap();
    let face = ttf_parser::Face::parse(&data, 0).unwrap();
    println!("glyphs {}", face.number_of_glyphs());
}
