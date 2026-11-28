fn main() {
    let path = std::env::args().nth(1).expect("font");
    let data = std::fs::read(path).unwrap();
    let face = ttf_parser::Face::parse(&data, 0).unwrap();
    println!("glyphs {}", face.number_of_glyphs());
    let out = mitex::convert_math(r"x = \frac{-b \pm \sqrt{b^2-4ac}}{2a} \sum_{i=1}^n i^2", None).unwrap();
    println!("{out}");
}
