//! The reader's pipeline: markdown in, drawn rows out.
//!
//! Three stages, deliberately separate.
//!
//! 1. [`parse`] turns the article's markdown into a [`Doc`]: a tree of
//!    blocks and styled runs, with every link numbered in document order. It
//!    knows nothing of widths or colours, so its output can be asserted by
//!    reading it.
//! 2. [`layout`] turns a `Doc`, a width and a theme into a [`Rendered`]: the
//!    `Line`s to draw, the [`LinkSpan`]s a click has to land on, the height
//!    a scrollbar needs, and the plain text `y` copies.
//! 3. [`cache`] keeps eight of those, keyed by the entry, when its text was
//!    last written, the width and which theme is up. Eight is `n` and `p`
//!    back and forth over a handful of articles, and a width change on top.
//!
//! The split is what makes the expensive half cheap: the parse is per
//! article and the layout is per article *per width*, and neither happens on
//! a frame where nothing moved.

//! Each stage is its own module and is named by it: `markdown::parse::parse`,
//! `markdown::layout::layout`, `markdown::cache::Cache`. No re-exports here
//! -- in a binary crate a `pub use` nothing has reached for yet is a
//! warning, and the window that reaches for these lands in the next work
//! package.

pub mod cache;
pub mod layout;
pub mod parse;

/// One article, with every block treatment in it, for the tests in this
/// module and for the fake core the window's own tests run against.
///
/// Written rather than taken from a real page, like everything else in
/// `testdata/`: a fixture that exercises a heading, bold, italic, inline
/// code, two links, a nested bulleted list, an ordered list, a fenced block
/// with a language, a quote, a rule, a three-column table with an aligned
/// column, an image and a footnote is not something a real article obliges
/// with, and one that did would be somebody's copyright.
#[cfg(test)]
pub const FIXTURE_MD: &str = r#"# Why the borrow checker says no: a field guide to lifetimes

The borrow checker is **not a wall**. It is a very *patient* reviewer that has
read the [nomicon](https://doc.rust-lang.org/nomicon/) and every `&'a str` in
your crate, and would like [a word](https://example.org/lifetimes) about three
of them.[^1]

## Three rules

- Every value has exactly one owner.
- You may borrow it, but not while it is being changed.
  - A shared borrow is many.
  - A mutable borrow is one.
- A borrow may not outlive what it borrows.

1. Name the lifetime.
2. Make the compiler agree with the name.

```rust
fn longest<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.len() > b.len() { a } else { b }
}
```

> Lifetimes are not about how long a value lives. They are about how long a
> reference is allowed to.

---

| Rule | What it means | Cost |
| --- | :---: | ---: |
| ownership | one owner per value | 0 |
| borrowing | many readers or one writer | 0 |

![a borrow checker diagram](https://example.org/diagram.png)

See also <https://doc.rust-lang.org/book/>.

[^1]: The three in the list above, which is the whole of it.
"#;
