/// This module is a modified version from Tokio
///
/// See https://docs.rs/tokio-macros/2.5.0/src/tokio_macros/entry.rs.html
use proc_macro2::{Span, TokenStream, TokenTree};
use quote::{ToTokens, quote, quote_spanned};
use syn::{
  Attribute, Ident, PatType, PathSegment, Signature, Visibility, braced,
  parse::{Parse, ParseStream, Parser},
  spanned::Spanned,
};

// syn::AttributeArgs does not implement syn::Parse
type AttributeArgs = syn::punctuated::Punctuated<syn::Meta, syn::Token![,]>;

#[derive(Clone, Copy, PartialEq)]
enum RuntimeFlavor {
  CurrentThread,
  Threaded,
}

impl RuntimeFlavor {
  fn from_str(s: &str) -> Result<RuntimeFlavor, String> {
    match s {
      "current_thread" => Ok(RuntimeFlavor::CurrentThread),
      "multi_thread" => Ok(RuntimeFlavor::Threaded),
      "single_thread" => {
        Err("The single threaded runtime flavor is called `current_thread`.".to_string())
      }
      "basic_scheduler" => Err(
        "The `basic_scheduler` runtime flavor has been renamed to `current_thread`.".to_string(),
      ),
      "threaded_scheduler" => Err(
        "The `threaded_scheduler` runtime flavor has been renamed to `multi_thread`.".to_string(),
      ),
      _ => Err(format!(
        "No such runtime flavor `{s}`. The runtime flavors are `current_thread` and `multi_thread`."
      )),
    }
  }
}

struct FinalConfig {
  flavor: RuntimeFlavor,
  worker_threads: Option<usize>,
  start_paused: Option<bool>,
  borrow_runtime: Option<PatType>,
}

impl FinalConfig {
  /// Config used in case of the attribute not being able to build a valid config
  fn error_config(input: &ItemFn) -> Self {
    let mut config = FinalConfig {
      flavor: RuntimeFlavor::CurrentThread,
      worker_threads: None,
      start_paused: None,
      borrow_runtime: None,
    };

    if let Ok(Some(ident)) = get_runtime_ident(&input, false) {
      config.borrow_runtime = Some(ident.to_owned());
    }

    config
  }
}

struct Configuration {
  rt_multi_thread_available: bool,
  default_flavor: RuntimeFlavor,
  flavor: Option<RuntimeFlavor>,
  worker_threads: Option<(usize, Span)>,
  start_paused: Option<(bool, Span)>,
  borrow_runtime: Option<PatType>,
  is_test: bool,
}

impl Configuration {
  fn new(is_test: bool, rt_multi_thread: bool) -> Self {
    Configuration {
      rt_multi_thread_available: rt_multi_thread,
      default_flavor: match is_test {
        true => RuntimeFlavor::CurrentThread,
        false => RuntimeFlavor::Threaded,
      },
      flavor: None,
      worker_threads: None,
      start_paused: None,
      borrow_runtime: None,
      is_test,
    }
  }

  fn set_flavor(&mut self, runtime: syn::Lit, span: Span) -> Result<(), syn::Error> {
    if self.flavor.is_some() {
      return Err(syn::Error::new(span, "`flavor` set multiple times."));
    }

    let runtime_str = parse_string(runtime, span, "flavor")?;
    let runtime =
      RuntimeFlavor::from_str(&runtime_str).map_err(|err| syn::Error::new(span, err))?;
    self.flavor = Some(runtime);
    Ok(())
  }

  fn set_worker_threads(&mut self, worker_threads: syn::Lit, span: Span) -> Result<(), syn::Error> {
    if self.worker_threads.is_some() {
      return Err(syn::Error::new(
        span,
        "`worker_threads` set multiple times.",
      ));
    }

    let worker_threads = parse_int(worker_threads, span, "worker_threads")?;
    if worker_threads == 0 {
      return Err(syn::Error::new(span, "`worker_threads` may not be 0."));
    }
    self.worker_threads = Some((worker_threads, span));
    Ok(())
  }

  fn set_start_paused(&mut self, start_paused: syn::Lit, span: Span) -> Result<(), syn::Error> {
    if self.start_paused.is_some() {
      return Err(syn::Error::new(span, "`start_paused` set multiple times."));
    }

    let start_paused = parse_bool(start_paused, span, "start_paused")?;
    self.start_paused = Some((start_paused, span));
    Ok(())
  }

  fn set_borrow_runtime(&mut self, pat_type: &PatType) -> Result<(), syn::Error> {
    if self.borrow_runtime.is_some() {
      return Err(syn::Error::new(
        pat_type.span(),
        "attempted to borrow runtime multiple times.",
      ));
    }

    self.borrow_runtime = Some(pat_type.to_owned());
    Ok(())
  }

  fn macro_name(&self) -> &'static str {
    if self.is_test {
      "async_local::test"
    } else {
      "async_local::main"
    }
  }

  fn build(&self) -> Result<FinalConfig, syn::Error> {
    use RuntimeFlavor as F;

    let flavor = self.flavor.unwrap_or(self.default_flavor);
    let worker_threads = match (flavor, self.worker_threads) {
      (F::CurrentThread, Some((_, worker_threads_span))) => {
        let msg = format!(
          "The `worker_threads` option requires the `multi_thread` runtime flavor. Use `#[{}(flavor = \"multi_thread\")]`",
          self.macro_name(),
        );
        return Err(syn::Error::new(worker_threads_span, msg));
      }
      (F::CurrentThread, None) => None,
      (F::Threaded, worker_threads) if self.rt_multi_thread_available => {
        worker_threads.map(|(val, _span)| val)
      }
      (F::Threaded, _) => {
        let msg = if self.flavor.is_none() {
          "The default runtime flavor is `multi_thread`, but the `rt-multi-thread` feature is disabled."
        } else {
          "The runtime flavor `multi_thread` requires the `rt-multi-thread` feature."
        };
        return Err(syn::Error::new(Span::call_site(), msg));
      }
    };

    let start_paused = match (flavor, self.start_paused) {
      (F::Threaded, Some((_, start_paused_span))) => {
        let msg = format!(
          "The `start_paused` option requires the `current_thread` runtime flavor. Use `#[{}(flavor = \"current_thread\")]`",
          self.macro_name(),
        );
        return Err(syn::Error::new(start_paused_span, msg));
      }
      (F::CurrentThread, Some((start_paused, _))) => Some(start_paused),
      (_, None) => None,
    };

    let borrow_runtime = self.borrow_runtime.clone();

    Ok(FinalConfig {
      flavor,
      worker_threads,
      start_paused,
      borrow_runtime,
    })
  }
}

fn parse_int(int: syn::Lit, span: Span, field: &str) -> Result<usize, syn::Error> {
  match int {
    syn::Lit::Int(lit) => match lit.base10_parse::<usize>() {
      Ok(value) => Ok(value),
      Err(e) => Err(syn::Error::new(
        span,
        format!("Failed to parse value of `{field}` as integer: {e}"),
      )),
    },
    _ => Err(syn::Error::new(
      span,
      format!("Failed to parse value of `{field}` as integer."),
    )),
  }
}

fn parse_string(int: syn::Lit, span: Span, field: &str) -> Result<String, syn::Error> {
  match int {
    syn::Lit::Str(s) => Ok(s.value()),
    syn::Lit::Verbatim(s) => Ok(s.to_string()),
    _ => Err(syn::Error::new(
      span,
      format!("Failed to parse value of `{field}` as string."),
    )),
  }
}

fn parse_bool(bool: syn::Lit, span: Span, field: &str) -> Result<bool, syn::Error> {
  match bool {
    syn::Lit::Bool(b) => Ok(b.value),
    _ => Err(syn::Error::new(
      span,
      format!("Failed to parse value of `{field}` as bool."),
    )),
  }
}

fn build_config(
  input: &ItemFn,
  args: AttributeArgs,
  is_test: bool,
  rt_multi_thread: bool,
) -> Result<FinalConfig, syn::Error> {
  let mut config = Configuration::new(is_test, rt_multi_thread);
  let macro_name = config.macro_name();

  for arg in args {
    match arg {
      syn::Meta::NameValue(namevalue) => {
        let ident = namevalue
          .path
          .get_ident()
          .ok_or_else(|| syn::Error::new_spanned(&namevalue, "Must have specified ident"))?
          .to_string()
          .to_lowercase();
        let lit = match &namevalue.value {
          syn::Expr::Lit(syn::ExprLit { lit, .. }) => lit,
          expr => return Err(syn::Error::new_spanned(expr, "Must be a literal")),
        };
        match ident.as_str() {
          "worker_threads" => {
            config.set_worker_threads(lit.clone(), syn::spanned::Spanned::span(lit))?;
          }
          "flavor" => {
            config.set_flavor(lit.clone(), syn::spanned::Spanned::span(lit))?;
          }
          "start_paused" => {
            config.set_start_paused(lit.clone(), syn::spanned::Spanned::span(lit))?;
          }
          "core_threads" => {
            let msg = "Attribute `core_threads` is renamed to `worker_threads`";
            return Err(syn::Error::new_spanned(namevalue, msg));
          }
          name => {
            let msg = format!(
              "Unknown attribute {name} is specified; expected one of: `flavor`, `worker_threads`, `start_paused`",
            );
            return Err(syn::Error::new_spanned(namevalue, msg));
          }
        }
      }
      syn::Meta::Path(path) => {
        let name = path
          .get_ident()
          .ok_or_else(|| syn::Error::new_spanned(&path, "Must have specified ident"))?
          .to_string()
          .to_lowercase();
        let msg = match name.as_str() {
          "threaded_scheduler" | "multi_thread" => {
            format!("Set the runtime flavor with #[{macro_name}(flavor = \"multi_thread\")].")
          }
          "basic_scheduler" | "current_thread" | "single_threaded" => {
            format!("Set the runtime flavor with #[{macro_name}(flavor = \"current_thread\")].")
          }
          "flavor" | "worker_threads" | "start_paused" => {
            format!("The `{name}` attribute requires an argument.")
          }
          name => {
            format!(
              "Unknown attribute {name} is specified; expected one of: `flavor`, `worker_threads`, `start_paused`."
            )
          }
        };
        return Err(syn::Error::new_spanned(path, msg));
      }
      other => {
        return Err(syn::Error::new_spanned(
          other,
          "Unknown attribute inside the macro",
        ));
      }
    }
  }

  match (get_runtime_ident(&input, true)?, &input.sig.asyncness) {
    (Some(pat), None) => {
      config.set_borrow_runtime(pat)?;
    }
    (Some(_), Some(token)) => {
      return Err(syn::Error::new(
        token.span(),
        "the `async` keyword cannot by used while borrowing the runtime",
      ));
    }
    (None, None) => {
      return Err(syn::Error::new_spanned(
        input.sig.fn_token,
        "the `async` keyword is missing from the function declaration",
      ));
    }
    _ => {}
  }

  config.build()
}

fn get_runtime_ident(input: &ItemFn, strict: bool) -> Result<Option<&PatType>, syn::Error> {
  let inputs = input.sig.inputs.iter().map(|fn_arg|
    match fn_arg {
      syn::FnArg::Receiver(receiver) => Err(syn::Error::new(
        receiver.span(),
        "function cannot have receiver",
      )),
      syn::FnArg::Typed(pat_type) => {
        if let syn::Type::Reference(type_reference) = pat_type.ty.as_ref() {
          if let syn::Type::Path(type_path) = type_reference.elem.as_ref() {
            let segments: Vec<&PathSegment> = type_path.path.segments.iter().collect();

            let runtime_segment = match segments.as_slice() {
              &[type_segment] if type_segment.ident.eq("Runtime") => type_segment,
              &[module, type_segment]
                if module.ident.eq("runtime") && type_segment.ident.eq(&"Runtime") =>
              {
                type_segment
              }
              &[crate_path, module, type_segment]
                if crate_path.ident.eq("tokio")
                  && module.ident.eq("runtime")
                  && type_segment.ident.eq("Runtime") =>
              {
                type_segment
              }
              _ => {
                return Err(syn::Error::new(
                  pat_type.span(),
                  "unsupported argument type specified",
                ));
              }
            };

            return match &runtime_segment.arguments {
                  syn::PathArguments::None => Ok((pat_type, pat_type.span())),
                  syn::PathArguments::AngleBracketed(angle_bracketed_generic_arguments) => {
                    let arguments_len  = angle_bracketed_generic_arguments.args.len();

                    if arguments_len.eq(&1) {
                      Err(syn::Error::new(pat_type.span(), format!("Runtime takes 0 generic arguments but 1 generic argument was supplied")))
                    } else {
                      Err(syn::Error::new(pat_type.span(), format!("Runtime takes 0 generic arguments but {arguments_len} generic arguments were supplied")))
                    }
                  },
                  syn::PathArguments::Parenthesized(_) => {
                    Err(syn::Error::new(pat_type.span(), format!("Runtime cannot have parenthesized type parameters")))
                  },
                }
            };
          }

          Err(syn::Error::new(
            fn_arg.span(),
            "unsupported argument type specified",
          ))
        }
      }
  );

  let mut runtime_pat = None;
  let mut error: Option<syn::Error> = None;

  for result in inputs {
    match result {
      Ok((pat, span)) => {
        if runtime_pat.is_some() {
          let err = syn::Error::new(span, "attempted to borrow runtime multiple times");

          if let Some(error) = &mut error {
            error.combine(err);
          } else {
            error = Some(err);
          }
        } else {
          if strict {
            runtime_pat = Some(pat);
          } else {
            return Ok(Some(pat));
          }
        }
      }
      Err(err) => {
        if let Some(error) = &mut error {
          error.combine(err);
        } else {
          error = Some(err);
        }
      }
    }
  }

  if let Some(err) = error {
    Err(err)
  } else {
    Ok(runtime_pat)
  }
}

fn parse_knobs(mut input: ItemFn, is_test: bool, config: FinalConfig) -> TokenStream {
  input.sig.asyncness = None;
  input.sig.inputs.clear();

  // If type mismatch occurs, the current rustc points to the last statement.
  let (last_stmt_start_span, last_stmt_end_span) = {
    let mut last_stmt = input.stmts.last().cloned().unwrap_or_default().into_iter();

    // `Span` on stable Rust has a limitation that only points to the first
    // token, not the whole tokens. We can work around this limitation by
    // using the first/last span of the tokens like
    // `syn::Error::new_spanned` does.
    let start = last_stmt.next().map_or_else(Span::call_site, |t| t.span());
    let end = last_stmt.last().map_or(start, |t| t.span());
    (start, end)
  };

  let crate_path = Ident::new("async_local", last_stmt_start_span).into_token_stream();

  let mut rt = match config.flavor {
    RuntimeFlavor::CurrentThread => quote_spanned! {last_stmt_start_span=>
        #crate_path::__runtime::Builder::new_current_thread()
    },
    RuntimeFlavor::Threaded => quote_spanned! {last_stmt_start_span=>
        #crate_path::__runtime::Builder::new_multi_thread()
    },
  };
  if let Some(v) = config.worker_threads {
    rt = quote_spanned! {last_stmt_start_span=> #rt.worker_threads(#v) };
  }
  if let Some(v) = config.start_paused {
    rt = quote_spanned! {last_stmt_start_span=> #rt.start_paused(#v) };
  }

  let generated_attrs = if is_test {
    quote! {
        #[::core::prelude::v1::test]
    }
  } else {
    quote! {}
  };

  let ensure_configured = if !is_test {
    quote_spanned! { last_stmt_start_span =>
        if module_path!().contains("::") {
          panic!("#[async_local::main] can only be used on the crate root main function");
        }

        #[async_local::linkme::distributed_slice(async_local::__runtime::RUNTIMES)]
        #[linkme(crate = async_local::linkme)]
        static RUNTIME_CONTEXT: async_local::__runtime::RuntimeContext = async_local::__runtime::RuntimeContext::Main;
    }
  } else {
    quote_spanned! { last_stmt_start_span =>
        #[async_local::linkme::distributed_slice(async_local::__runtime::RUNTIMES)]
        #[linkme(crate = async_local::linkme)]
        static RUNTIME_CONTEXT: async_local::__runtime::RuntimeContext = async_local::__runtime::RuntimeContext::Test;
    }
  };

  let body_ident = quote! { body };

  let run = if config.borrow_runtime.is_some() {
    quote! {
      return unsafe {
        runtime.run(#body_ident)
      };
    }
  } else {
    quote! {
      return unsafe {
        runtime.block_on(#body_ident)
      };
    }
  };

  // This explicit `return` is intentional. See tokio-rs/tokio#4636
  let last_block = quote_spanned! {last_stmt_end_span=>
      #[allow(clippy::expect_used, clippy::diverging_sub_expression, clippy::needless_return)]
      {
        #ensure_configured

        let runtime = #rt
            .enable_all()
            .build()
            .expect("Failed building the Runtime");

        #run
      }
  };

  let body = input.body();

  let body = if let Some(ident) = config.borrow_runtime {
    quote! {
        let body = |#ident| #body;
    }
  }
  // For test functions pin the body to the stack and use `Pin<&mut dyn
  // Future>` to reduce the amount of `Runtime::block_on` (and related
  // functions) copies we generate during compilation due to the generic
  // parameter `F` (the future to block on). This could have an impact on
  // performance, but because it's only for testing it's unlikely to be very
  // large.
  //
  // We don't do this for the main function as it should only be used once so
  // there will be no benefit.
  else if is_test {
    let output_type = match &input.sig.output {
      // For functions with no return value syn doesn't print anything,
      // but that doesn't work as `Output` for our boxed `Future`, so
      // default to `()` (the same type as the function output).
      syn::ReturnType::Default => quote! { () },
      syn::ReturnType::Type(_, ret_type) => quote! { #ret_type },
    };
    quote! {
        let body = async #body;
        #crate_path::pin!(body);
        let body: ::core::pin::Pin<&mut dyn ::core::future::Future<Output = #output_type>> = body;
    }
  } else {
    quote! {
        let body = async #body;
    }
  };

  input.into_tokens(generated_attrs, body, last_block)
}

fn token_stream_with_error(mut tokens: TokenStream, error: syn::Error) -> TokenStream {
  tokens.extend(error.into_compile_error());
  tokens
}

pub(crate) fn main(args: TokenStream, item: TokenStream, rt_multi_thread: bool) -> TokenStream {
  // If any of the steps for this macro fail, we still want to expand to an item that is as close
  // to the expected output as possible. This helps out IDEs such that completions and other
  // related features keep working.
  let input: ItemFn = match syn::parse2(item.clone()) {
    Ok(it) => it,
    Err(e) => return token_stream_with_error(item, e),
  };

  let config = if input.sig.ident != "main" {
    Err(syn::Error::new_spanned(
      &input.sig.ident,
      "macro can only be used on the root main function",
    ))
  } else {
    AttributeArgs::parse_terminated
      .parse2(args)
      .and_then(|args| build_config(&input, args, false, rt_multi_thread))
  };

  match config {
    Ok(config) => parse_knobs(input, false, config),
    Err(e) => {
      let config = FinalConfig::error_config(&input);
      token_stream_with_error(parse_knobs(input, false, config), e)
    }
  }
}

// Check whether given attribute is a test attribute of forms:
// * `#[test]`
// * `#[core::prelude::*::test]` or `#[::core::prelude::*::test]`
// * `#[std::prelude::*::test]` or `#[::std::prelude::*::test]`
fn is_test_attribute(attr: &Attribute) -> bool {
  let path = match &attr.meta {
    syn::Meta::Path(path) => path,
    _ => return false,
  };
  let candidates = [
    ["core", "prelude", "*", "test"],
    ["std", "prelude", "*", "test"],
  ];
  if path.leading_colon.is_none()
    && path.segments.len() == 1
    && path.segments[0].arguments.is_none()
    && path.segments[0].ident == "test"
  {
    return true;
  } else if path.segments.len() != candidates[0].len() {
    return false;
  }
  candidates.into_iter().any(|segments| {
    path
      .segments
      .iter()
      .zip(segments)
      .all(|(segment, path)| segment.arguments.is_none() && (path == "*" || segment.ident == path))
  })
}

pub(crate) fn test(args: TokenStream, item: TokenStream, rt_multi_thread: bool) -> TokenStream {
  // If any of the steps for this macro fail, we still want to expand to an item that is as close
  // to the expected output as possible. This helps out IDEs such that completions and other
  // related features keep working.
  let input: ItemFn = match syn::parse2(item.clone()) {
    Ok(it) => it,
    Err(e) => return token_stream_with_error(item, e),
  };
  let config = if let Some(attr) = input.attrs().find(|attr| is_test_attribute(attr)) {
    let msg = "second test attribute is supplied, consider removing or changing the order of your test attributes";
    Err(syn::Error::new_spanned(attr, msg))
  } else {
    AttributeArgs::parse_terminated
      .parse2(args)
      .and_then(|args| build_config(&input, args, true, rt_multi_thread))
  };

  match config {
    Ok(config) => parse_knobs(input, true, config),
    Err(e) => {
      let config = FinalConfig::error_config(&input);
      token_stream_with_error(parse_knobs(input, true, config), e)
    }
  }
}

struct ItemFn {
  outer_attrs: Vec<Attribute>,
  vis: Visibility,
  sig: Signature,
  brace_token: syn::token::Brace,
  inner_attrs: Vec<Attribute>,
  stmts: Vec<proc_macro2::TokenStream>,
}

impl ItemFn {
  /// Access all attributes of the function item.
  fn attrs(&self) -> impl Iterator<Item = &Attribute> {
    self.outer_attrs.iter().chain(self.inner_attrs.iter())
  }

  /// Get the body of the function item in a manner so that it can be
  /// conveniently used with the `quote!` macro.
  fn body(&self) -> Body<'_> {
    Body {
      brace_token: self.brace_token,
      stmts: &self.stmts,
    }
  }

  /// Convert our local function item into a token stream.
  fn into_tokens(
    self,
    generated_attrs: proc_macro2::TokenStream,
    body: proc_macro2::TokenStream,
    last_block: proc_macro2::TokenStream,
  ) -> TokenStream {
    let mut tokens = proc_macro2::TokenStream::new();
    // Outer attributes are simply streamed as-is.
    for attr in self.outer_attrs {
      attr.to_tokens(&mut tokens);
    }

    // Inner attributes require extra care, since they're not supported on
    // blocks (which is what we're expanded into) we instead lift them
    // outside of the function. This matches the behavior of `syn`.
    for mut attr in self.inner_attrs {
      attr.style = syn::AttrStyle::Outer;
      attr.to_tokens(&mut tokens);
    }

    // Add generated macros at the end, so macros processed later are aware of them.
    generated_attrs.to_tokens(&mut tokens);

    self.vis.to_tokens(&mut tokens);
    self.sig.to_tokens(&mut tokens);

    self.brace_token.surround(&mut tokens, |tokens| {
      body.to_tokens(tokens);
      last_block.to_tokens(tokens);
    });

    tokens
  }
}

impl Parse for ItemFn {
  #[inline]
  fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
    // This parse implementation has been largely lifted from `syn`, with
    // the exception of:
    // * We don't have access to the plumbing necessary to parse inner
    //   attributes in-place.
    // * We do our own statements parsing to avoid recursively parsing
    //   entire statements and only look for the parts we're interested in.

    let outer_attrs = input.call(Attribute::parse_outer)?;
    let vis: Visibility = input.parse()?;
    let sig: Signature = input.parse()?;

    let content;
    let brace_token = braced!(content in input);
    let inner_attrs = Attribute::parse_inner(&content)?;

    let mut buf = proc_macro2::TokenStream::new();
    let mut stmts = Vec::new();

    while !content.is_empty() {
      if let Some(semi) = content.parse::<Option<syn::Token![;]>>()? {
        semi.to_tokens(&mut buf);
        stmts.push(buf);
        buf = proc_macro2::TokenStream::new();
        continue;
      }

      // Parse a single token tree and extend our current buffer with it.
      // This avoids parsing the entire content of the sub-tree.
      buf.extend([content.parse::<TokenTree>()?]);
    }

    if !buf.is_empty() {
      stmts.push(buf);
    }

    Ok(Self {
      outer_attrs,
      vis,
      sig,
      brace_token,
      inner_attrs,
      stmts,
    })
  }
}

struct Body<'a> {
  brace_token: syn::token::Brace,
  // Statements, with terminating `;`.
  stmts: &'a [TokenStream],
}

impl ToTokens for Body<'_> {
  fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
    self.brace_token.surround(tokens, |tokens| {
      for stmt in self.stmts {
        stmt.to_tokens(tokens);
      }
    });
  }
}
