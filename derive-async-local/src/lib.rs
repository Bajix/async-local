use proc_macro2::Span;
use quote::quote;
use syn::{
  Data, DeriveInput, GenericArgument, PathArguments, Type, TypePath, parse::Error,
  parse_macro_input,
};

mod entry;

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
    .map(|(_, type_path)| Error::new_spanned(type_path, "Context cannot be wrapped in a pointer type nor cell type and must not be invalidated nor repurposed until dropped"))
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

/// Marks async function to be executed by the selected runtime.
///
/// # Non-worker async function
///
/// Note that the async function marked with this macro does not run as a
/// worker. The expectation is that other tasks are spawned by the function here.
/// Awaiting on other futures from the function provided here will not
/// perform as fast as those spawned as workers.
///
/// # Multi-threaded runtime
///
/// To use the multi-threaded runtime, the macro can be configured using
///
/// ```
/// #[async_local::main(flavor = "multi_thread", worker_threads = 10)]
/// # async fn main() {}
/// ```
///
/// The `worker_threads` option configures the number of worker threads, and
/// defaults to the number of cpus on the system. This is the default flavor.
///
/// Note: The multi-threaded runtime requires the `rt-multi-thread` feature
/// flag.
///
/// # Current thread runtime
///
/// To use the single-threaded runtime known as the `current_thread` runtime,
/// the macro can be configured using
///
/// ```
/// #[async_local::main(flavor = "current_thread")]
/// # async fn main() {}
/// ```
/// ## Usage
///
/// ### Using the multi-thread runtime
///
/// ```rust
/// #[async_local::main]
/// async fn main() {
///   println!("Hello world");
/// }
/// ```
///
/// ### Using current thread runtime
///
/// The basic scheduler is single-threaded.
///
/// ```rust
/// #[async_local::main(flavor = "current_thread")]
/// async fn main() {
///   println!("Hello world");
/// }
/// ```
///
/// ### Set number of worker threads
///
/// ```rust
/// #[async_local::main(worker_threads = 2)]
/// async fn main() {
///   println!("Hello world");
/// }
/// ```
///
/// ### Configure the runtime to start with time paused
///
/// ```rust
/// #[async_local::main(flavor = "current_thread", start_paused = true)]
/// async fn main() {
///   println!("Hello world");
/// }
/// ```
///
/// Note that `start_paused` requires the `test-util` feature to be enabled.
#[proc_macro_attribute]
pub fn main(
  args: proc_macro::TokenStream,
  item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
  entry::main(args.into(), item.into(), true).into()
}

/// Marks async function to be executed by runtime, suitable to test environment.
///
/// Note: This macro is designed to be simplistic and targets applications that
/// do not require a complex setup. If the provided functionality is not
/// sufficient, you may be interested in using
/// [Builder](../tokio/runtime/struct.Builder.html), which provides a more
/// powerful interface.
///
/// # Multi-threaded runtime
///
/// To use the multi-threaded runtime, the macro can be configured using
///
/// ```no_run
/// #[async_local::test(flavor = "multi_thread", worker_threads = 1)]
/// async fn my_test() {
///   assert!(true);
/// }
/// ```
///
/// The `worker_threads` option configures the number of worker threads, and
/// defaults to the number of cpus on the system.
///
/// Note: The multi-threaded runtime requires the `rt-multi-thread` feature
/// flag.
///
/// # Current thread runtime
///
/// The default test runtime is single-threaded. Each test gets a
/// separate current-thread runtime.
///
/// ```no_run
/// #[async_local::test]
/// async fn my_test() {
///   assert!(true);
/// }
/// ```
///
/// ## Usage
///
/// ### Using the multi-thread runtime
///
/// ```no_run
/// #[async_local::test(flavor = "multi_thread")]
/// async fn my_test() {
///   assert!(true);
/// }
/// ```
///
/// ### Using current thread runtime
///
/// ```no_run
/// #[async_local::test]
/// async fn my_test() {
///   assert!(true);
/// }
/// ```
///
/// ### Set number of worker threads
///
/// ```no_run
/// #[async_local::test(flavor = "multi_thread", worker_threads = 2)]
/// async fn my_test() {
///   assert!(true);
/// }
/// ```
///
/// ### Configure the runtime to start with time paused
///
/// ```no_run
/// #[async_local::test(start_paused = true)]
/// async fn my_test() {
///   assert!(true);
/// }
/// ```
///
/// Note that `start_paused` requires the `test-util` feature to be enabled.
#[proc_macro_attribute]
pub fn test(
  args: proc_macro::TokenStream,
  item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
  entry::test(args.into(), item.into(), true).into()
}
