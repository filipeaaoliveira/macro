//! Where each function in a Rust file ends, and what its body holds.
//!
//! `cargo crap`'s JSON gives a function's first line only. It counts a
//! function from its `fn` keyword to its body's closing brace; this finds the
//! same spans, so a function's rows can be met against the diff, and keeps
//! each body's tokens, so a function moved or renamed is known by its body.

use std::collections::BTreeMap;

use quote::ToTokens;
use syn::visit::{self, Visit};
use syn::{Block, ImplItemFn, ItemFn, Signature, TraitItemFn};

#[cfg(test)]
mod test;

/// One function of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    /// Its last line: its body's closing brace.
    pub last: u32,
    /// Its body's tokens, without whitespace or comments.
    pub body: String,
}

/// Each function, by its first line (its `fn` keyword's).
pub fn functions(source: &str) -> Result<BTreeMap<u32, Function>, syn::Error> {
    let file = syn::parse_file(source)?;
    let mut functions = Functions::default();
    functions.visit_file(&file);
    Ok(functions.0)
}

#[derive(Default)]
struct Functions(BTreeMap<u32, Function>);

impl Functions {
    fn add(&mut self, sig: &Signature, block: &Block) {
        let first = sig.fn_token.span.start().line;
        let last = block.brace_token.span.close().end().line;
        let function = Function {
            last: line_number(last),
            body: block.to_token_stream().to_string(),
        };
        self.0.insert(line_number(first), function);
    }
}

/// A line number as the diff's hunks hold it; no Rust file reaches
/// `u32::MAX` lines.
fn line_number(line: usize) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}

impl<'ast> Visit<'ast> for Functions {
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
