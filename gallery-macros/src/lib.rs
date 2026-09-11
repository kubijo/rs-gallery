//! The `#[scene]` inventory registration attribute.

use heck::ToTitleCase;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    FnArg, Ident, ItemFn, LitStr, ReturnType, Token, Type,
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
    if func.sig.asyncness.is_some() {
        return syn::Error::new_spanned(&func.sig, "a scene cannot be async")
            .to_compile_error()
            .into();
    }
    let returns_unit = match &func.sig.output {
        ReturnType::Default => true,
        ReturnType::Type(_, output) => {
            matches!(output.as_ref(), Type::Tuple(tuple) if tuple.elems.is_empty())
        }
    };
    if !returns_unit {
        return syn::Error::new_spanned(&func.sig.output, "a scene must return `()`")
            .to_compile_error()
            .into();
    }
    let render = match func.sig.inputs.len() {
        2 => quote! { #ident },
        3 => {
            let Some(FnArg::Typed(globals)) = func.sig.inputs.iter().nth(2) else {
                return syn::Error::new_spanned(
                    &func.sig.inputs,
                    "a scene's third argument must be `globals: &Globals`",
                )
                .to_compile_error()
                .into();
            };
            let Type::Reference(reference) = globals.ty.as_ref() else {
                return syn::Error::new_spanned(
                    &globals.ty,
                    "a scene's third argument must be an immutable reference",
                )
                .to_compile_error()
                .into();
            };
            if reference.mutability.is_some() {
                return syn::Error::new_spanned(
                    &globals.ty,
                    "catalog globals are read-only while a scene renders",
                )
                .to_compile_error()
                .into();
            }
            let adapter = format_ident!("__gallery_render_{ident}");
            quote! {
                #adapter
            }
        }
        _ => {
            return syn::Error::new_spanned(
                &func.sig.inputs,
                "a scene takes `(ctx, ui)` or `(ctx, ui, globals: &Globals)`",
            )
            .to_compile_error()
            .into();
        }
    };
    let adapter = if func.sig.inputs.len() == 3 {
        let FnArg::Typed(globals) = func.sig.inputs.iter().nth(2).expect("third argument") else {
            unreachable!("validated above")
        };
        let Type::Reference(reference) = globals.ty.as_ref() else {
            unreachable!("validated above")
        };
        let ty = reference.elem.as_ref();
        let adapter = format_ident!("__gallery_render_{ident}");
        quote! {
            #[doc(hidden)]
            fn #adapter(
                ctx: &mut ::gallery::SceneCtx<'_>,
                ui: &mut ::gallery::egui::Ui,
            ) {
                let globals: crate::__GalleryGlobals = ctx.__gallery_globals::<#ty>();
                #ident(ctx, ui, &globals);
            }
        }
    } else {
        quote! {}
    };
    quote! {
        #func
        #adapter
        ::gallery::inventory::submit! {
            ::gallery::SceneEntry {
                render: #render,
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
