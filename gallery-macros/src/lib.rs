//! The `#[scene]` attribute for gallery. Registers a `fn(&mut egui::Ui)` as a scene via `inventory`,
//! keyed by its `module_path!()`. The name defaults to the title-cased function name; `default` marks
//! it as its group's default (collapsing a single-scene group in the sidebar); `order = N` sorts it
//! within the group.

use heck::ToTitleCase;
use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Ident, ItemFn, LitStr, Token,
    parse::{Parse, ParseStream},
    parse_macro_input,
};

/// `#[scene]`,
/// `#[scene("name")]`,
/// `#[scene(default)]`,
/// `#[scene("name", default)]`,
/// `#[scene(order = N)]`.
struct Args {
    name: Option<String>,
    default: bool,
    /// Sort position within the group; unset sorts last, by name.
    order: u32,
}

impl Parse for Args {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = if input.peek(LitStr) {
            let lit = input.parse::<LitStr>()?.value();
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
            Some(lit)
        } else {
            None
        };

        let mut default = false;
        let mut order = u32::MAX;
        while !input.is_empty() {
            let kw: Ident = input.parse()?;
            if kw == "default" {
                default = true;
            } else if kw == "order" {
                input.parse::<Token![=]>()?;
                order = input.parse::<syn::LitInt>()?.base10_parse()?;
            } else {
                return Err(syn::Error::new(
                    kw.span(),
                    "expected `default` or `order = N`",
                ));
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Self {
            name,
            default,
            order,
        })
    }
}

#[proc_macro_attribute]
pub fn scene(attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);
    let ident = &func.sig.ident;
    let args = if attr.is_empty() {
        Args {
            name: None,
            default: false,
            order: u32::MAX,
        }
    } else {
        match syn::parse::<Args>(attr) {
            Ok(args) => args,
            Err(e) => return e.to_compile_error().into(),
        }
    };
    let name = args
        .name
        .unwrap_or_else(|| ident.to_string().to_title_case());
    let default = args.default;
    let order = args.order;
    // The scene's own source, for the shell's Source tab.
    let source = {
        let file: syn::File = syn::parse_quote! { #func };
        prettyplease::unparse(&file)
    };
    quote! {
        #func
        ::gallery::inventory::submit! {
            ::gallery::SceneEntry {
                render: #ident,
                name: #name,
                module_path: ::core::module_path!(),
                default: #default,
                order: #order,
                source: #source,
            }
        }
    }
    .into()
}
