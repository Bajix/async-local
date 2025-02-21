use proc_macro2::Span;
use quote::quote;
use syn::{
  Data, DeriveInput, GenericArgument, PathArguments, Type, TypePath, parse::Error,
  parse_macro_input,
};

fn is_context(type_path: &TypePath) -> bool {
  let segments: Vec<_> = type_path
    .path
    .segments
    .iter()
    .map(|segment| segment.ident.to_string())
    .collect();

  matches!(
    *segments
      .iter()
      .map(String::as_ref)
      .collect::<Vec<&str>>()
      .as_slice(),
    ["async_local", "Context"] | ["Context"]
  )
}

/// Derive [AsRef](https://doc.rust-lang.org/std/convert/trait.AsRef.html)<[`Context<T>`](https://docs.rs/async-local/latest/async_local/struct.Context.html)> and [`AsContext`](https://docs.rs/async-local/latest/async_local/trait.AsContext.html) for a struct
#[proc_macro_derive(AsContext)]
pub fn derive_as_context(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
  let input = parse_macro_input!(input as DeriveInput);
  let ident = &input.ident;
  let (impl_generics, ty_generics, where_clause) = &input.generics.split_for_impl();

  if let Some(err) = input
    .generics
    .lifetimes()
    .map(|lifetime| Error::new_spanned(lifetime, "cannot derive AsContext with lifetimes"))
    .reduce(|mut err, other| {
      err.combine(other);
      err
    })
  {
    return err.into_compile_error().into();
  }

  let data_struct = if let Data::Struct(data_struct) = &input.data {
    data_struct
  } else {
    return Error::new(Span::call_site(), "can only derive AsContext on structs")
      .into_compile_error()
      .into();
  };

  let path_fields: Vec<_> = data_struct
    .fields
    .iter()
    .filter_map(|field| {
      if let Type::Path(type_path) = &field.ty {
        Some((field, type_path))
      } else {
        None
      }
    })
    .collect();

  let wrapped_context_error = path_fields
    .iter()
    .filter(|(_, type_path)| {
      if let Some(segment) = type_path.path.segments.last() {
        if let PathArguments::AngleBracketed(inner) = &segment.arguments {
          if let Some(GenericArgument::Type(Type::Path(type_path))) = inner.args.first() {
            return is_context(type_path);
          }
        }
      }
      false
    })
    .map(|(_, type_path)| Error::new_spanned(type_path, "Context must upholds the pin drop guarantee: it cannot be wrapped in a pointer type nor cell type and it must not be invalidated nor repurposed until dropped"))
    .reduce(|mut err, other| {
      err.combine(other);
      err
    });

  if let Some(err) = wrapped_context_error {
    return err.into_compile_error().into();
  }

  let context_paths: Vec<_> = path_fields
    .iter()
    .filter(|(_, type_path)| is_context(type_path))
    .collect();

  if context_paths.len().eq(&0) {
    return Error::new(Span::call_site(), "struct must use Context exactly once")
      .into_compile_error()
      .into();
  }

  if context_paths.len().gt(&1) {
    return context_paths
      .into_iter()
      .map(|(_, type_path)| Error::new_spanned(type_path, "Context cannot be used more than once"))
      .reduce(|mut err, other| {
        err.combine(other);
        err
      })
      .unwrap()
      .into_compile_error()
      .into();
  }

  let (field, type_path) = context_paths.into_iter().next().unwrap();

  let context_ident = &field.ident;

  let ref_type = type_path.path.segments.last().and_then(|segment| {
    if let PathArguments::AngleBracketed(ref_type) = &segment.arguments {
      Some(&ref_type.args)
    } else {
      None
    }
  });

  let expanded = quote!(
    impl #impl_generics AsRef<#type_path> for #ident #ty_generics #where_clause {
      fn as_ref(&self) -> &#type_path {
        &self.#context_ident
      }
    }

    unsafe impl #impl_generics async_local::AsContext for #ident #ty_generics #where_clause {
      type Target = #ref_type;
    }
  );

  expanded.into()
}
