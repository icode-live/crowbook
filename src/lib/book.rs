use error::{Error,Result};
use cleaner::{Cleaner, French};
use parser::Parser;
use token::Token;
use epub::EpubRenderer;
use html::HtmlRenderer;
use latex::LatexRenderer;
use odt::OdtRenderer;
use templates::{epub,html,epub3};
use escape;

use std::fs::File;
use std::io::{Write,Read};
use std::env;
use std::path::Path;
use std::borrow::Cow;

use mustache;
use mustache::MapBuilder;

/// Numbering for a given chapter
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Number {
    Hidden, // chapter's title is hidden
    Unnumbered, // chapter is not numbered
    Default, // chapter follows books numbering, number is given automatically
    Specified(i32), //chapter number set to specified number
}
    
// Configuration of the book
#[derive(Debug)]
pub struct Book {
    // Metadata
    pub lang: String,
    pub author: String,
    pub title: String,
    pub description: Option<String>,
    pub subject: Option<String>,
    pub cover: Option<String>,

    // Output files
    pub output_epub: Option<String>,
    pub output_html: Option<String>,
    pub output_pdf: Option<String>,
    pub output_tex: Option<String>,
    pub output_odt: Option<String>,
    pub temp_dir: String,

    // internal structure
    pub chapters: Vec<(Number, Vec<Token>)>, 

    // options
    pub numbering: bool, // turns on/off chapter numbering (individual chapters may still avoid it)
    pub autoclean: bool, 
    pub nb_char: char,
    pub numbering_template: String, // template for chapter numbering
    pub verbose: bool,

    // for latex
    pub tex_command: String,

    // for epub
    pub epub_css: Option<String>,
    pub epub_template: Option<String>,
    pub epub_version: u8,

    // for HTML
    pub html_template: Option<String>,
    pub html_css: Option<String>,
}

impl Book {
    // Creates a new Book with default options
    pub fn new() -> Book {
        Book {
            verbose: false,
            numbering: true,
            autoclean: true,
            chapters: vec!(),
            lang: String::from("en"),
            author: String::from("Anonymous"),
            title: String::from("Untitled"),
            description: None,
            subject: None,
            cover: None,
            nb_char: ' ',
            numbering_template: String::from("{{number}}. {{title}}"),
            temp_dir: String::from("."),
            output_epub: None,
            output_html: None,
            output_pdf: None,
            output_tex: None,
            output_odt: None,
            tex_command: String::from("pdflatex"),
            epub_css: None,
            epub_template: None,
            epub_version: 2,
            html_template: None,
            html_css: None,
        }
    }

    /// Creates a new book from a file
    ///
    /// This method also changes the current directory to the one of this file
    pub fn new_from_file(filename: &str) -> Result<Book> {
        let path = Path::new(filename);
        let mut f = try!(File::open(&path).map_err(|_| Error::FileNotFound(String::from(filename))));

        // change current directory
        if let Some(parent) = path.parent() {
            if !parent.to_string_lossy().is_empty() {
                if !env::set_current_dir(&parent).is_ok() {
                    return Err(Error::ConfigParser("could not change current directory to the one of the config file",
                                                   format!("{}", parent.display())));
                }
            }
        }

        
        let mut s = String::new();

        try!(f.read_to_string(&mut s).map_err(|_| Error::ConfigParser("file contains invalid UTF-8, could not parse it",
                                                                      String::from(filename))));
        let mut book = Book::new();
        try!(book.set_from_config(&s));
        Ok(book)
    }

    /// Returns a MapBuilder, to be used (and completed) for templating
    pub fn get_mapbuilder(&self, format: &str) -> MapBuilder {
        fn clone(x:&str) -> String {
            x.to_owned()
        }
        let f:fn(&str)->String = match format {
            "none" => clone,
            "html" => escape::escape_html,
            "tex" => escape::escape_tex,
            _ => panic!("get mapbuilder called with invalid escape format")
        };
        MapBuilder::new()
            .insert_str("author", f(&self.author))
            .insert_str("title", f(&self.title))
            .insert_str("lang", self.lang.clone())
    }

    /// Return a Box<Cleaner> corresponding to the appropriate cleaning method, or None
    pub fn get_cleaner(&self) -> Option<Box<Cleaner>> {
        if self.autoclean {
            let lang = self.lang.to_lowercase();
            if lang.starts_with("fr") {
                Some(Box::new(French::new(self.nb_char)))
            } else {
                Some(Box::new(()))
            }
        } else {
            None
        }
    }

    /// Returns the string corresponding to a number, title, and the numbering template
    pub fn get_header(&self, n: i32, title: &str) -> Result<String> {
        let template = mustache::compile_str(&self.numbering_template);
        let data = MapBuilder::new()
            .insert_str("title", String::from(title))
            .insert_str("number", format!("{}", n))
            .build();
        let mut res:Vec<u8> = vec!();
        template.render_data(&mut res, &data);
        match String::from_utf8(res) {
            Err(_) => Err(Error::Render("header generated by mustache was not valid utf-8")),
            Ok(res) => Ok(res)
        }
    }

    /// Sets options according to configuration file
    ///
    /// A line with "option: value" sets the option to value
    /// + chapter_name.md adds the (default numbered) chapter
    /// - chapter_name.md adds the (unnumbered) chapter
    /// 3. chapter_name.md adds the (custom numbered) chapter
    pub fn set_from_config(&mut self, s: &str) -> Result<()> {
        fn get_char(s: &str) -> Result<char> {
            let words: Vec<_> = s.trim().split('\'').collect();
            if words.len() != 3 {
                return Err(Error::ConfigParser("could not parse char", String::from(s)));
            }
            let chars: Vec<_> = words[1].chars().collect();
            if chars.len() != 1 {
                return Err(Error::ConfigParser("could not parse char", String::from(s)));
            }
            Ok(chars[0])
        }
        
        fn get_filename(s: &str) -> Result<&str> {
            let words:Vec<&str> = (&s[1..]).split_whitespace().collect();
            if words.len() > 1 {
                return Err(Error::ConfigParser("chapter filenames must not contain whitespace", String::from(s)));
            } else if words.len() < 1 {
                return Err(Error::ConfigParser("no chapter name specified", String::from(s)));
            }
            Ok(words[0])
        }
        
        for line in s.lines() {
            let line = line.trim();
            let bool_error = |_| Error::ConfigParser("could not parse bool", String::from(line));
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('-') {
                //unnumbered chapter
                let file = try!(get_filename(line));
                try!(self.add_chapter(Number::Unnumbered, file));
            } else if line.starts_with('+') {
                //nunmbered chapter
                let file = try!(get_filename(line));
                try!(self.add_chapter(Number::Default, file));
            } else if line.starts_with('!') {
                // hidden chapter
                let file = try!(get_filename(line));
                try!(self.add_chapter(Number::Hidden, file));
            } else if line.starts_with(|c: char| c.is_digit(10)) {
                // chapter with specific number
                let parts:Vec<_> = line.splitn(2, |c: char| c == '.' || c == ':' || c == '+').collect();
                if parts.len() != 2 {
                    return Err(Error::ConfigParser("ill-formatted line specifying chapter number", String::from(line)));
                } else {
                    let file = try!(get_filename(parts[1]));
                    let number = try!(parts[0].parse::<i32>().map_err(|_| Error::ConfigParser("Error parsing integer", String::from(line))));
                    try!(self.add_chapter(Number::Specified(number), file));
                }
            } else {
                // standard case: "option: value"
                let parts:Vec<_> = line.splitn(2, ':').collect();
                if parts.len() != 2 {
                    return Err(Error::ConfigParser("option setting must be of the form option: value", String::from(line)));
                }
                let option = parts[0].trim();
                let value = parts[1].trim();
                match option {
                    "nb-char" | "nb_char" => self.nb_char = try!(get_char(value)),
                    "numbering-template" | "numbering_template" => self.numbering_template = String::from(value),
                    "verbose" => self.verbose = try!(value.parse::<bool>().map_err(bool_error)),
                    "numbering" => self.numbering = try!(value.parse::<bool>().map_err(bool_error)),
                    "autoclean" => self.autoclean = try!(value.parse::<bool>().map_err(bool_error)),
                    "temp_dir" | "temp-dir" => self.temp_dir = String::from(value),
                    "output_epub" | "output-epub" => self.output_epub = Some(String::from(value)),
                    "output_html" | "output-html" => self.output_html = Some(String::from(value)),
                    "output_tex" | "output-tex" => self.output_tex = Some(String::from(value)),
                    "output_pdf" | "output-pdf" => self.output_pdf = Some(String::from(value)),
                    "output_odt" | "output-odt" => self.output_odt = Some(String::from(value)),
                    "tex_command" | "tex-command" => self.tex_command = String::from(value),
                    "author" => self.author = String::from(value),
                    "title" => self.title = String::from(value),
                    "cover" => self.cover = Some(String::from(value)),
                    "lang" => self.lang = String::from(value),
                    "description" => self.description = Some(String::from(value)),
                    "subject" => self.subject = Some(String::from(value)),
                    "epub_css" | "epub-css" => self.epub_css = Some(String::from(value)),
                    "epub_template" | "epub-template" => self.epub_template = Some(String::from(value)),
                    "epub_version" | "epub-version" => self.epub_version = match value {
                        "2" => 2,
                        "3" => 3,
                        _ => return Err(Error::ConfigParser("epub_version must either be 2 or 3", String::from(value))),
                    },
                    "html_template" | "html-template" => self.html_template = Some(String::from(value)),
                    "html_css" | "html-css" => self.html_css = Some(String::from(value)),
                    _ => return Err(Error::ConfigParser("unrecognized option", String::from(line))),
                }
            }
        }

        Ok(())
    }
    
    /// Render book to pdf according to book options
    pub fn render_pdf(&self, file: &str) -> Result<()> {
        if self.verbose {
            println!("Attempting to generate pdf...");
        }
        let mut latex = LatexRenderer::new(&self);
        let result = try!(latex.render_pdf());
        if self.verbose {
            println!("{}", result);
        }
        println!("Successfully generated pdf file: {}", file);
        Ok(())
    }

    /// Render book to epub according to book options
    pub fn render_epub(&self) -> Result<()> {
        if self.verbose {
            println!("Attempting to generate epub...");
        }
        let mut epub = EpubRenderer::new(&self);
        let result = try!(epub.render_book());
        if self.verbose {
            println!("{}", result);
        }
        println!("Successfully generated epub file: {}", self.output_epub.as_ref().unwrap());
        Ok(())
    }

        /// Render book to odt according to book options
    pub fn render_odt(&self) -> Result<()> {
        if self.verbose {
            println!("Attempting to generate Odt...");
        }
        let mut odt = OdtRenderer::new(&self);
        let result = try!(odt.render_book());
        if self.verbose {
            println!("{}", result);
        }
        println!("Successfully generated odt file: {}", self.output_odt.as_ref().unwrap());
        Ok(())
    }

    /// Render book to html according to book options
    pub fn render_html(&self, file: &str) -> Result<()> {
        if self.verbose {
            println!("Attempting to generate HTML...");
        }
        let mut html = HtmlRenderer::new(&self);
        let result = try!(html.render_book());
        let mut f = try!(File::create(file).map_err(|_| Error::Render("could not create HTML file")));
        try!(f.write_all(&result.as_bytes()).map_err(|_| Error::Render("problem when writing to HTML file")));
        println!("Successfully generated HTML file: {}", file);
        Ok(())
    }

    /// Render book to pdf according to book options
    pub fn render_tex(&self, file: &str) -> Result<()> {
        if self.verbose {
            println!("Attempting to generate LaTeX...");
        }
        let mut latex = LatexRenderer::new(&self);
        let result = try!(latex.render_book());
        let mut f = try!(File::create(file).map_err(|_| Error::Render("could not create LaTeX file")));
        try!(f.write_all(&result.as_bytes()).map_err(|_| Error::Render("problem when writing to LaTeX file")));
        println!("Successfully generated LaTeX file: {}", file);
        Ok(())
    }
        
    /// Generates output files acccording to book options
    pub fn render_all(&self) -> Result<()> {
        let mut did_some_stuff = false;

        if self.output_epub.is_some() {
            did_some_stuff = true;
            try!(self.render_epub());
        }

        if let Some(ref file) = self.output_html {
            did_some_stuff = true;
            try!(self.render_html(file));
        }
        if let Some(ref file) = self.output_tex {
            did_some_stuff = true;
            try!(self.render_tex(file));
        }
        if let Some(ref file) = self.output_pdf {
            did_some_stuff = true;
            try!(self.render_pdf(file));
        }

        if self.output_odt.is_some() {
            did_some_stuff = true;
            try!(self.render_odt());
        }
        if !did_some_stuff {
            println!("Warning: generated no file because no output file speficied. Add output_{{format}} to your config file.");
        }
        Ok(())
    }

    
    /// File: location of the file for this chapter
    pub fn add_chapter(&mut self, number: Number, file: &str) -> Result<()> {
        let mut parser = Parser::new();
        if let Some(cleaner) = self.get_cleaner() {
            parser = parser.with_cleaner(cleaner)
        }
        let v = try!(parser.parse_file(file));
        self.chapters.push((number, v));
        Ok(())
    }

    /// Returns the template (default or modified version)
    pub fn get_template(&self, template: &str) -> Result<Cow<'static, str>> {
        let (option, fallback) = match template {
            "epub_css" => (&self.epub_css, epub::CSS),
            "epub_template" => (&self.epub_template,
                                if self.epub_version == 3 {epub3::TEMPLATE} else {epub::TEMPLATE}),
            "html_css" => (&self.html_css, html::CSS),
            "html_template" => (&self.html_template, html::TEMPLATE),
            _ => return Err(Error::ConfigParser("invalid template", template.to_owned())),
        };
        if let Some (ref s) = *option {
            let mut f = try!(File::open(s).map_err(|_| Error::FileNotFound(s.to_owned())));
            let mut res = String::new();
            try!(f.read_to_string(&mut res)
                 .map_err(|_| Error::ConfigParser("file could not be read", s.to_owned())));
            Ok(Cow::Owned(res))
        } else {
            Ok(Cow::Borrowed(fallback))
        }
    }
}
