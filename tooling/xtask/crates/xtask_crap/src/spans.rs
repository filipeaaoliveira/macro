//! Where each function in a Rust file ends.
//!
//! `cargo crap`'s JSON gives a function's first line only. It counts a
//! function from its `fn` keyword to its body's closing brace; this finds the
//! same spans, so a function's rows can be met against the diff.

use std::collections::BTreeMap;

use syn::visit::{self, Visit};
use syn::{Block, ImplItemFn, ItemFn, Signature, TraitItemFn};

#[cfg(test)]
mod test;

/// Each function's last line, by its first line (its `fn` keyword's).
pub fn function_spans(source: &str) -> Result<BTreeMap<u32, u32>, syn::Error> {
    let file = syn::parse_file(source)?;
    let mut spans = Spans::default();
    spans.visit_file(&file);
    Ok(spans.0)
}

#[derive(Default)]
struct Spans(BTreeMap<u32, u32>);

impl Spans {
    fn add(&mut self, sig: &Signature, block: &Block) {
        let first = sig.fn_token.span.start().line;
        let last = block.brace_token.span.close().end().line;
        self.0.insert(line_number(first), line_number(last));
    }
}

/// A line number as the diff's hunks hold it; no Rust file reaches
/// `u32::MAX` lines.
fn line_number(line: usize) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}

impl<'ast> Visit<'ast> for Spans {
    fn visit_item_fn(&mut self, item: &'ast ItemFn) {
        self.add(&item.sig, &item.block);
        visit::visit_item_fn(self, item);
    }

    fn visit_impl_item_fn(&mut self, item: &'ast ImplItemFn) {
        self.add(&item.sig, &item.block);
        visit::visit_impl_item_fn(self, item);
    }

    fn visit_trait_item_fn(&mut self, item: &'ast TraitItemFn) {
        if let Some(block) = &item.default {
            self.add(&item.sig, block);
        }
        visit::visit_trait_item_fn(self, item);
    }
}
