//! Trusted Q03 syntax contract. This executable never compiles candidate code.
use serde_json::{json, Value};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, FnArg, GenericArgument, Item, ItemFn, Lit, Meta, Pat, ReturnType, Token, Type,
    Visibility,
};

const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

fn item_attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

// Evaluate only facts known with cfg(test)=false. Unknown features/platforms
// remain production code; cfg(any(test, feature="x")) cannot hide literals.
fn without_test(meta: &Meta) -> Option<bool> {
    match meta {
        Meta::Path(path) if path.is_ident("test") => Some(false),
        Meta::List(list) => {
            let arguments = Punctuated::<Meta, Token![,]>::parse_terminated
                .parse2(list.tokens.clone())
                .ok()?;
            let values: Vec<_> = arguments.iter().map(without_test).collect();
            if list.path.is_ident("not") && values.len() == 1 {
                values[0].map(|value| !value)
            } else if list.path.is_ident("all") {
                if values.contains(&Some(false)) {
                    Some(false)
                } else if values.iter().all(|value| *value == Some(true)) {
                    Some(true)
                } else {
                    None
                }
            } else if list.path.is_ident("any") {
                if values.contains(&Some(true)) {
                    Some(true)
                } else if values.iter().all(|value| *value == Some(false)) {
                    Some(false)
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

fn production(attributes: &[Attribute]) -> bool {
    !attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && attribute
                .parse_args::<Meta>()
                .ok()
                .as_ref()
                .and_then(without_test)
                == Some(false)
    })
}

fn plain_type(ty: &Type) -> &Type {
    match ty {
        Type::Paren(ty) => plain_type(&ty.elem),
        Type::Group(ty) => plain_type(&ty.elem),
        _ => ty,
    }
}

fn is_u32(ty: &Type) -> bool {
    matches!(plain_type(ty), Type::Path(ty) if ty.qself.is_none() && ty.path.is_ident("u32"))
}

fn private_rate_table(function: &ItemFn) -> bool {
    let signature = &function.sig;
    if !matches!(function.vis, Visibility::Inherited)
        || signature.constness.is_some()
        || signature.asyncness.is_some()
        || signature.unsafety.is_some()
        || signature.abi.is_some()
        || signature.variadic.is_some()
        || !signature.generics.params.is_empty()
        || signature.generics.where_clause.is_some()
        || signature.inputs.len() != 1
    {
        return false;
    }
    let Some(FnArg::Typed(argument)) = signature.inputs.first() else {
        return false;
    };
    if !matches!(&*argument.pat, Pat::Ident(pat) if pat.ident == "region" && pat.by_ref.is_none() && pat.subpat.is_none())
    {
        return false;
    }
    let Type::Reference(reference) = plain_type(&argument.ty) else {
        return false;
    };
    if reference.mutability.is_some()
        || !matches!(plain_type(&reference.elem), Type::Path(ty) if ty.qself.is_none() && ty.path.is_ident("str"))
    {
        return false;
    }
    let ReturnType::Type(_, result) = &signature.output else {
        return false;
    };
    let Type::Path(result) = plain_type(result) else {
        return false;
    };
    let names: Vec<_> = result
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect();
    if result.qself.is_some()
        || !matches!(
            names
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice(),
            ["Option"] | ["std", "option", "Option"] | ["core", "option", "Option"]
        )
    {
        return false;
    }
    let Some(segment) = result.path.segments.last() else {
        return false;
    };
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    let Some(GenericArgument::Type(tuple)) = arguments.args.first() else {
        return false;
    };
    matches!(plain_type(tuple), Type::Tuple(tuple) if arguments.args.len() == 1 && tuple.elems.len() == 2 && tuple.elems.iter().all(is_u32))
}

fn plain_expression(expression: &Expr) -> &Expr {
    match expression {
        Expr::Paren(expression) => plain_expression(&expression.expr),
        Expr::Group(expression) => plain_expression(&expression.expr),
        _ => expression,
    }
}

#[derive(Default)]
struct ProductionVisitor {
    local: usize,
    remote: usize,
    calls_table: bool,
    skip_nested_functions: bool,
}

impl ProductionVisitor {
    fn macro_literals(&mut self, mut cursor: syn::buffer::Cursor<'_>) {
        while !cursor.eof() {
            if let Some((literal, rest)) = cursor.literal() {
                // syn leaves macro bodies unexpanded. Parse actual literal
                // tokens as ExprLit rather than matching their source text.
                if let Ok(expression) = syn::parse_str::<syn::ExprLit>(&literal.to_string()) {
                    self.visit_expr_lit(&expression);
                }
                cursor = rest;
            } else if let Some((group, _, _, rest)) = cursor.any_group() {
                self.macro_literals(group);
                cursor = rest;
            } else if let Some((_, rest)) = cursor.token_tree() {
                cursor = rest;
            } else {
                break;
            }
        }
    }
}

impl<'ast> Visit<'ast> for ProductionVisitor {
    fn visit_item(&mut self, item: &'ast Item) {
        if production(item_attributes(item)) {
            visit::visit_item(self, item);
        }
    }

    fn visit_item_fn(&mut self, function: &'ast ItemFn) {
        if !self.skip_nested_functions {
            visit::visit_item_fn(self, function);
        }
    }

    fn visit_attribute(&mut self, _attribute: &'ast Attribute) {
        // Documentation and attribute strings are not executable literals.
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        let attributes = match item {
            syn::ImplItem::Const(item) => &item.attrs,
            syn::ImplItem::Fn(item) => &item.attrs,
            syn::ImplItem::Type(item) => &item.attrs,
            syn::ImplItem::Macro(item) => &item.attrs,
            _ => return,
        };
        if production(attributes) {
            visit::visit_impl_item(self, item);
        }
    }

    fn visit_trait_item(&mut self, item: &'ast syn::TraitItem) {
        let attributes = match item {
            syn::TraitItem::Const(item) => &item.attrs,
            syn::TraitItem::Fn(item) => &item.attrs,
            syn::TraitItem::Type(item) => &item.attrs,
            syn::TraitItem::Macro(item) => &item.attrs,
            _ => return,
        };
        if production(attributes) {
            visit::visit_trait_item(self, item);
        }
    }

    fn visit_macro(&mut self, source: &'ast syn::Macro) {
        let buffer = syn::buffer::TokenBuffer::new2(source.tokens.clone());
        self.macro_literals(buffer.begin());
    }

    fn visit_expr_lit(&mut self, expression: &'ast syn::ExprLit) {
        if !production(&expression.attrs) {
            return;
        }
        if let Lit::Str(literal) = &expression.lit {
            match literal.value().as_str() {
                "local" => self.local += 1,
                "remote" => self.remote += 1,
                _ => {}
            }
        }
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Expr::Path(function) = plain_expression(&call.func) {
            let names: Vec<_> = function
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect();
            let names: Vec<_> = names.iter().map(String::as_str).collect();
            if function.qself.is_none()
                && matches!(names.as_slice(), ["rate_table"] | ["self", "rate_table"] | ["crate", "rate_table"])
                && call.args.len() == 1
                && call.args.first().is_some_and(|argument| matches!(plain_expression(argument), Expr::Path(argument) if argument.qself.is_none() && argument.path.is_ident("region")))
            {
                self.calls_table = true;
            }
        }
        visit::visit_expr_call(self, call);
    }
}

fn inspect(source: &str) -> Result<Value, syn::Error> {
    let file = syn::parse_file(source)?;
    let helpers: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(function)
                if function.sig.ident == "rate_table" && production(&function.attrs) =>
            {
                Some(function)
            }
            _ => None,
        })
        .collect();
    let shipping: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(function)
                if function.sig.ident == "shipping_rate"
                    && matches!(function.vis, Visibility::Public(_))
                    && production(&function.attrs) =>
            {
                Some(function)
            }
            _ => None,
        })
        .collect();
    let mut all = ProductionVisitor::default();
    all.visit_file(&file);
    let mut shipping_body = ProductionVisitor {
        skip_nested_functions: true,
        ..Default::default()
    };
    for function in &shipping {
        shipping_body.visit_block(&function.block);
    }
    let mut mapping_body = ProductionVisitor {
        skip_nested_functions: true,
        ..Default::default()
    };
    for function in &helpers {
        mapping_body.visit_block(&function.block);
    }
    let checks = json!({
        "private_rate_table": helpers.len() == 1 && private_rate_table(helpers[0]) && mapping_body.local == 1 && mapping_body.remote == 1,
        "single_local_literal": all.local == 1,
        "single_remote_literal": all.remote == 1,
        "shipping_calls_table": shipping.len() == 1 && shipping_body.calls_table,
        "shipping_no_region_literals": shipping.len() == 1 && shipping_body.local == 0 && shipping_body.remote == 0,
    });
    let passed = checks
        .as_object()
        .unwrap()
        .values()
        .all(|value| value == true);
    Ok(json!({
        "status": if passed { "PASS" } else { "FAIL" },
        "checks": checks,
        "scope": "explicit Q03 refactor contract; Rust syn AST, recursive cfg(test) exclusion, exact function bodies; behavior evaluated independently"
    }))
}

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 1 {
        println!(
            "{}",
            json!({"status":"ERROR","error":"expected one UTF-8 Rust source file path"})
        );
        return ExitCode::from(2);
    }
    let result = (|| -> Result<Value, &'static str> {
        let mut file =
            File::open(Path::new(&arguments[0])).map_err(|_| "source could not be opened")?;
        if !file
            .metadata()
            .map_err(|_| "source metadata unavailable")?
            .is_file()
        {
            return Err("source must be an ordinary file");
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_SOURCE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "source could not be read")?;
        if bytes.len() as u64 > MAX_SOURCE_BYTES {
            return Err("source exceeds 2 MiB limit");
        }
        let source = std::str::from_utf8(&bytes).map_err(|_| "source is not UTF-8")?;
        inspect(source).map_err(|_| "source is not valid Rust syntax")
    })();
    match result {
        Ok(result) => {
            let code = if result["status"] == "PASS" { 0 } else { 1 };
            println!("{result}");
            ExitCode::from(code)
        }
        Err(error) => {
            println!("{}", json!({"status":"ERROR","error":error}));
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELPER: &str = "fn rate_table(region: &str) -> Option<(u32,u32)> { match region { \"local\" => Some((5,9)), \"remote\" => Some((12,20)), _ => None } }\n";
    const SHIPPING: &str = "pub fn shipping_rate(region: &str, expedited: bool) -> Option<u32> { rate_table(region).map(|(standard,fast)| if expedited {fast} else {standard}) }\n";

    fn checked(source: &str, check: &str) -> bool {
        inspect(source).unwrap()["checks"][check].as_bool().unwrap()
    }

    #[test]
    fn helper_order_does_not_change_acceptance() {
        for source in [format!("{HELPER}{SHIPPING}"), format!("{SHIPPING}{HELPER}")] {
            assert_eq!(inspect(&source).unwrap()["status"], "PASS");
        }
    }

    #[test]
    fn comments_documentation_and_raw_braces_are_not_function_boundaries() {
        let noise = "// } fn rate_table \"local\"\n#[doc=\"remote\"]\nconst NOISE:&str=r###\"} fn shipping_rate { \\\"local\\\"\"###;\n";
        assert_eq!(
            inspect(&format!("{noise}{SHIPPING}{HELPER}")).unwrap()["status"],
            "PASS"
        );
    }

    #[test]
    fn recursive_cfg_test_items_are_excluded() {
        let tests = "#[cfg(test)] mod first { const LOCAL:&str=\"local\"; } mod nested { #[cfg(all(test,unix))] fn sample() { let _=\"remote\"; } }\n";
        assert_eq!(
            inspect(&format!("{tests}{SHIPPING}{HELPER}")).unwrap()["status"],
            "PASS"
        );
    }

    #[test]
    fn private_helper_must_have_exact_signature_and_mapping() {
        for helper in [
            String::new(),
            HELPER.replace("fn rate_table", "pub fn rate_table"),
            HELPER.replace("&str", "String"),
            HELPER.replace("(u32,u32)", "(u64,u32)"),
            "fn rate_table(region: &str) -> Option<(u32,u32)> { None }\n".into(),
        ] {
            assert!(!checked(
                &format!("{helper}{SHIPPING}"),
                "private_rate_table"
            ));
        }
    }

    #[test]
    fn production_duplicates_and_macro_literals_fail() {
        for duplicate in [
            "const DUPLICATE:&str=\"local\";",
            "fn noisy(){ println!(\"local\"); }",
        ] {
            assert!(!checked(
                &format!("{HELPER}{SHIPPING}{duplicate}"),
                "single_local_literal"
            ));
        }
    }

    #[test]
    fn shipping_body_cannot_contain_region_literals() {
        let shipping = SHIPPING.replace("{ rate_table", "{ let _=\"remote\"; rate_table");
        assert!(!checked(
            &format!("{HELPER}{shipping}"),
            "shipping_no_region_literals"
        ));
    }

    #[test]
    fn table_name_in_comment_or_later_function_is_not_delegation() {
        let shipping = "pub fn shipping_rate(region:&str,expedited:bool)->Option<u32>{ // rate_table(region)\nNone }\n";
        assert!(!checked(
            &format!("{shipping}{HELPER}"),
            "shipping_calls_table"
        ));
    }

    #[test]
    fn unknown_cfg_features_do_not_hide_production_duplicates() {
        let hidden = "#[cfg(any(test,feature=\"production\"))] const DUPLICATE:&str=\"local\";";
        assert!(!checked(
            &format!("{HELPER}{SHIPPING}{hidden}"),
            "single_local_literal"
        ));
    }
}
