//! Common macros for Sequoia's FFI crates.

use std::collections::HashMap;

extern crate lazy_static;
use lazy_static::lazy_static;
extern crate syn;
use syn::spanned::Spanned;
extern crate quote;
extern crate proc_macro;
extern crate proc_macro2;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;

use quote::{quote, ToTokens};

/// Wraps a function's body in a catch_unwind block, aborting on
/// panics.
///
/// Unwinding the stack across the FFI boundary is [undefined
/// behavior].  We therefore need to wrap every FFI function's body
/// with [catch_unwind].  This macro does that in an unobtrusive
/// manner.
///
/// [undefined behavior]: https://doc.rust-lang.org/nomicon/unwinding.html
/// [catch_unwind]: https://doc.rust-lang.org/std/panic/fn.catch_unwind.html
///
/// # Example
///
/// ```rust,ignore
/// #[ffi_catch_abort]
/// #[no_mangle]
/// pub extern "system" fn sahnetorte() {
///     assert_eq!(2 * 3, 4);  // See what happens...
/// }
/// ```
#[proc_macro_attribute]
pub fn ffi_catch_abort(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse tokens into a function declaration.
    let fun = syn::parse_macro_input!(item as syn::ItemFn);

    // Extract all information from the parsed function that we need
    // to compose the new function.
    let attrs = fun.attrs.iter()
        .fold(TokenStream2::new(),
              |mut acc, attr| {
                  acc.extend(attr.clone().into_token_stream());
                  acc
              });
    let vis = &fun.vis;
    let constness = &fun.constness;
    let unsafety = &fun.unsafety;
    let asyncness = &fun.asyncness;
    let abi = &fun.abi;
    let ident = &fun.ident;

    let decl = &fun.decl;
    let fn_token = &decl.fn_token;
    let fn_generics = &decl.generics;
    let fn_out = &decl.output;

    let mut fn_params = TokenStream2::new();
    decl.paren_token.surround(&mut fn_params, |ts| decl.inputs.to_tokens(ts));

    let block = &fun.block;

    // We wrap the functions body into an catch_unwind, asserting that
    // all variables captured by the closure are unwind safe.  This is
    // safe because we terminate the process on panics, therefore no
    // inconsistencies can be observed.
    let expanded = quote! {
        #attrs #vis #constness #unsafety #asyncness #abi
        #fn_token #ident #fn_generics #fn_params #fn_out
        {
            match ::std::panic::catch_unwind(
                ::std::panic::AssertUnwindSafe(|| #fn_out #block))
            {
                Ok(v) => v,
                Err(p) => {
                    unsafe {
                        ::libc::abort();
                    }
                },
            }
        }
    };

    // To debug problems with the generated code, just eprintln it:
    //
    // eprintln!("{}", expanded);

    expanded.into()
}

/// Derives FFI functions for a wrapper type.
///
/// # Example
///
/// ```rust,ignore
/// /// Holds a fingerprint.
/// #[::ffi_wrapper_type(prefix = "pgp_",
///                      derive = "Clone, Debug, Display, PartialEq, Hash")]
/// pub struct Fingerprint(openpgp::Fingerprint);
/// ```
#[proc_macro_attribute]
pub fn ffi_wrapper_type(args: TokenStream, input: TokenStream) -> TokenStream {
    // Parse tokens into a function declaration.
    let args = syn::parse_macro_input!(args as syn::AttributeArgs);
    let st = syn::parse_macro_input!(input as syn::ItemStruct);

    let mut name = None;
    let mut prefix = None;
    let mut derive = Vec::new();

    for arg in args.iter() {
        match arg {
            syn::NestedMeta::Meta(syn::Meta::NameValue(ref mnv)) => {
                let value = match mnv.lit {
                    syn::Lit::Str(ref s) => s.value(),
                    _ => unreachable!(),
                };
                match mnv.ident.to_string().as_ref() {
                    "name" => name = Some(value),
                    "prefix" => prefix = Some(value),
                    "derive" => {
                        for ident in value.split(",").map(|d| d.trim()
                                                          .to_string()) {
                            if let Some(f) = derive_functions().get::<str>(&ident) {
                                derive.push(f);
                            } else {
                                return syn::Error::new(
                                    mnv.ident.span(),
                                    format!("unknown derive: {}", ident))
                                    .to_compile_error().into();
                            }
                        }
                    },
                    name => return
                        syn::Error::new(mnv.ident.span(),
                                        format!("unexpected parameter: {}",
                                                name))
                        .to_compile_error().into(),
                }
            },
            _ => return syn::Error::new(arg.span(),
                                        "expected key = \"value\" pair")
                .to_compile_error().into(),
        }
    }

    let name = name.unwrap_or(ident2c(&st.ident));
    let prefix = prefix.unwrap_or("".into());

    // Parse the wrapped type.
    let wrapped_type = match &st.fields {
        syn::Fields::Unnamed(fields) => {
            if fields.unnamed.len() != 1 {
                return
                    syn::Error::new(st.fields.span(),
                                    "expected a single field")
                    .to_compile_error().into();
            }
            fields.unnamed.first().unwrap().value().ty.clone()
        },
        _ => return
            syn::Error::new(st.fields.span(),
                            format!("expected tuple struct, try: {}(...)",
                            st.ident))
            .to_compile_error().into(),
    };

    let default_derives: &[DeriveFn] = &[
        derive_free,
    ];
    let mut impls = TokenStream2::new();
    for dfn in derive.into_iter().chain(default_derives.iter()) {
        impls.extend(dfn(st.span(), &prefix, &name, &wrapped_type));
    }

    let expanded = quote! {
        #st

        // The derived functions.
        #impls
    };

    // To debug problems with the generated code, just eprintln it:
    //
    // eprintln!("{}", expanded);

    expanded.into()
}

/// Derives the C type from the Rust type.
fn ident2c(ident: &syn::Ident) -> String {
    let mut s = String::new();
    for (i, c) in ident.to_string().chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                s += "_";
            }
            s += &c.to_lowercase().to_string();
        } else {
            s += &c.to_string();
        }
    }
    s
}

#[test]
fn ident2c_tests() {
    let span = proc_macro2::Span::call_site();
    assert_eq!(&ident2c(&syn::Ident::new("Fingerprint", span)), "fingerprint");
    assert_eq!(&ident2c(&syn::Ident::new("PacketPile", span)), "packet_pile");
}

/// Describes our custom derive functions.
type DeriveFn = fn(proc_macro2::Span, &str, &str, &syn::Type) -> TokenStream2;

/// Maps trait names to our generator functions.
fn derive_functions() -> &'static HashMap<&'static str, DeriveFn>
{
    lazy_static! {
        static ref MAP: HashMap<&'static str, DeriveFn> = {
            let mut h = HashMap::new();
            h.insert("Clone", derive_clone as DeriveFn);
            h.insert("PartialEq", derive_equal as DeriveFn);
            h.insert("Hash", derive_hash as DeriveFn);
            h.insert("Display", derive_to_string as DeriveFn);
            h.insert("Debug", derive_debug as DeriveFn);
            h
        };
    }
    &MAP
}

/// Derives prefix_name_free.
fn derive_free(span: proc_macro2::Span, prefix: &str, name: &str,
               ty: &syn::Type)
               -> TokenStream2
{
    let ident = syn::Ident::new(&format!("{}{}_free", prefix, name),
                                span);
    quote! {
        /// Frees this object.
        #[::ffi_catch_abort] #[no_mangle]
        pub extern "system" fn #ident (this: Option<&mut #ty>) {
            if let Some(ptr) = this {
                unsafe {
                    drop(Box::from_raw(ptr))
                }
            }
        }
    }
}

/// Derives prefix_name_clone.
fn derive_clone(span: proc_macro2::Span, prefix: &str, name: &str,
                ty: &syn::Type)
                -> TokenStream2
{
    let ident = syn::Ident::new(&format!("{}{}_clone", prefix, name),
                                span);
    quote! {
        /// Clones this object.
        #[::ffi_catch_abort] #[no_mangle]
        pub extern "system" fn #ident (this: *const #ty)
                                       -> *mut #ty {
            let this = ffi_param_ref!(this);
            box_raw!(this.clone())
        }
    }
}

/// Derives prefix_name_equal.
fn derive_equal(span: proc_macro2::Span, prefix: &str, name: &str,
                ty: &syn::Type)
                -> TokenStream2
{
    let ident = syn::Ident::new(&format!("{}{}_equal", prefix, name),
                                span);
    quote! {
        /// Compares objects.
        #[::ffi_catch_abort] #[no_mangle]
        pub extern "system" fn #ident (a: *const #ty,
                                       b: *const #ty)
                                       -> bool {
            let a = ffi_param_ref!(a);
            let b = ffi_param_ref!(b);
            a == b
        }
    }
}


/// Derives prefix_name_to_string.
fn derive_to_string(span: proc_macro2::Span, prefix: &str, name: &str,
                    ty: &syn::Type)
                    -> TokenStream2
{
    let ident = syn::Ident::new(&format!("{}{}_to_string", prefix, name),
                                span);
    quote! {
        /// Returns a human readable description of this object
        /// intended for communication with end users.
        #[::ffi_catch_abort] #[no_mangle]
        pub extern "system" fn #ident (this: *const #ty)
                                       -> *mut ::libc::c_char {
            let this = ffi_param_ref!(this);
            ffi_return_string!(format!("{}", this))
        }
    }
}

/// Derives prefix_name_debug.
fn derive_debug(span: proc_macro2::Span, prefix: &str, name: &str,
                ty: &syn::Type)
                -> TokenStream2
{
    let ident = syn::Ident::new(&format!("{}{}_debug", prefix, name),
                                span);
    quote! {
        /// Returns a human readable description of this object
        /// suitable for debugging.
        #[::ffi_catch_abort] #[no_mangle]
        pub extern "system" fn #ident (this: *const #ty)
                                       -> *mut ::libc::c_char {
            let this = ffi_param_ref!(this);
            ffi_return_string!(format!("{:?}", this))
        }
    }
}

/// Derives prefix_name_hash.
fn derive_hash(span: proc_macro2::Span, prefix: &str, name: &str,
               ty: &syn::Type)
               -> TokenStream2
{
    let ident = syn::Ident::new(&format!("{}{}_hash", prefix, name),
                                span);
    quote! {
        /// Hashes this object.
        #[::ffi_catch_abort] #[no_mangle]
        pub extern "system" fn #ident (this: *const #ty)
                                       -> ::libc::uint64_t {
            use ::std::hash::{Hash, Hasher};

            let this = ffi_param_ref!(this);
            let mut hasher = ::build_hasher();
            this.hash(&mut hasher);
            hasher.finish()
        }
    }
}
