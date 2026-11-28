use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::layout::{Frame, FrameItem};
use typst_layout::PagedDocument;
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};

struct W {
    lib: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    main: Source,
}

impl World for W {
    fn library(&self) -> &LazyHash<Library> {
        &self.lib
    }
    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }
    fn main(&self) -> FileId {
        self.main.id()
    }
    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::AccessDenied)
        }
    }
    fn file(&self, _id: FileId) -> FileResult<Bytes> {
        Err(FileError::AccessDenied)
    }
    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }
    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

fn count(frame: &Frame) -> usize {
    frame
        .items()
        .map(|(_, it)| match it {
            FrameItem::Group(g) => count(&g.frame),
            FrameItem::Text(t) => t.glyphs.len(),
            _ => 1,
        })
        .sum()
}

fn main() {
    let path = std::env::args().nth(1).expect("font");
    let data = std::fs::read(path).unwrap();
    let face = ttf_parser::Face::parse(&data, 0).unwrap();
    println!("glyphs {}", face.number_of_glyphs());
    let fonts: Vec<Font> = Font::iter(Bytes::new(data)).collect();
    let book = FontBook::from_fonts(&fonts);
    let id = RootedPath::new(VirtualRoot::Project, VirtualPath::new("main.typ").unwrap()).intern();
    let src = "#set page(width: auto, height: auto, margin: 0pt)\n#set text(font: \"STIX Two Math\")\n$ x = (-b plus.minus sqrt(b^2 - 4 a c)) / (2 a) $\n$ sum_(i=1)^n i^2 $";
    let w = W {
        lib: LazyHash::new(Library::default()),
        book: LazyHash::new(book),
        fonts,
        main: Source::new(id, src.into()),
    };
    let t = std::time::Instant::now();
    let doc: PagedDocument = typst::compile(&w).output.expect("compile");
    println!("items {} in {:?}", count(&doc.pages()[0].frame), t.elapsed());
}
