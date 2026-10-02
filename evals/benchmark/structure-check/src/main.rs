//! Independent syntax contract: parses source; never builds candidate code.
use serde_json::json;
use std::{fs::File, io::Read, process::ExitCode};
use syn::visit::{self, Visit};
use syn::{Expr, ImplItem, Item, ItemFn, Member, Stmt, Type, Visibility};

fn production(attrs: &[syn::Attribute]) -> bool {
    !attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Meta>()
                .is_ok_and(|meta| matches!(meta, syn::Meta::Path(path) if path.is_ident("test")))
    })
}
fn parameter(function: &syn::Signature) -> Option<String> {
    function
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(arg) => match &*arg.pat {
                syn::Pat::Ident(p) => Some(p.ident.to_string()),
                _ => None,
            },
            _ => None,
        })
        .next()
}
fn path(expr: &Expr, name: &str) -> bool {
    matches!(expr, Expr::Path(p) if p.qself.is_none() && p.path.is_ident(name))
}
fn tail(block: &syn::Block) -> Option<&Expr> {
    if block.stmts.len() != 1 {
        return None;
    }
    match &block.stmts[0] {
        Stmt::Expr(Expr::Return(r), _) => r.expr.as_deref(),
        Stmt::Expr(expr, None) => Some(expr),
        _ => None,
    }
}
fn helper_wrapper(function: &ItemFn, helper: &str) -> bool {
    let Some(argument) = parameter(&function.sig) else {
        return false;
    };
    matches!(tail(&function.block), Some(Expr::Call(call)) if path(&call.func, helper)
        && call.args.len() == 1 && path(&call.args[0], &argument))
}
fn helper_contract(file: &syn::File, helper: &str, wrappers: [&str; 2]) -> bool {
    let functions: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(f) if production(&f.attrs) => Some(f),
            _ => None,
        })
        .collect();
    let helpers: Vec<_> = functions.iter().filter(|f| f.sig.ident == helper).collect();
    if helpers.len() != 1 {
        return false;
    }
    let f = helpers[0];
    let signature_ok = matches!(f.vis, Visibility::Inherited)
        && f.sig.inputs.len() == 1
        && f.sig.generics.params.is_empty()
        && f.sig.asyncness.is_none()
        && f.sig.unsafety.is_none()
        && matches!(&f.sig.output, syn::ReturnType::Type(_, ty) if matches!(&**ty, Type::Path(p) if p.path.is_ident("String")))
        && matches!(f.sig.inputs.first(), Some(syn::FnArg::Typed(arg)) if matches!(&*arg.ty, Type::Reference(r) if r.mutability.is_none() && matches!(&*r.elem, Type::Path(p) if p.path.is_ident("str"))));
    signature_ok
        && wrappers.iter().all(|name| {
            let found: Vec<_> = functions.iter().filter(|f| f.sig.ident == *name).collect();
            found.len() == 1
                && matches!(found[0].vis, Visibility::Public(_))
                && helper_wrapper(found[0], helper)
        })
}
fn standard_btree(file: &syn::File, ty: &Type) -> bool {
    let Type::Path(ty) = ty else {
        return false;
    };
    let names: Vec<_> = ty
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    if ty.qself.is_some() {
        return false;
    }
    if names == ["std", "collections", "BTreeMap"] {
        return true;
    }
    if names != ["BTreeMap"] {
        return false;
    }
    // The unqualified spelling is accepted only with the exact standard import.
    file.items.iter().any(|item| {
        matches!(item, Item::Use(u) if matches!(&u.tree,
        syn::UseTree::Path(a) if a.ident == "std" && matches!(&*a.tree,
            syn::UseTree::Path(b) if b.ident == "collections" && matches!(&*b.tree,
                syn::UseTree::Name(c) if c.ident == "BTreeMap"))))
    }) && !file.items.iter().any(|item| {
        matches!(item, Item::Type(t) if t.ident == "BTreeMap")
            || matches!(item, Item::Struct(s) if s.ident == "BTreeMap")
    })
}
fn indexed_lookup(expr: &Expr, field: &str, argument: &str) -> bool {
    match expr {
        Expr::MethodCall(call)
            if (call.method == "copied" || call.method == "cloned") && call.args.is_empty() =>
        {
            indexed_lookup(&call.receiver, field, argument)
        }
        Expr::MethodCall(call) if call.method == "get" && call.args.len() == 1 => {
            let key = match &call.args[0] {
                Expr::Reference(r) if r.mutability.is_none() => &*r.expr,
                e => e,
            };
            path(key, argument)
                && matches!(&*call.receiver, Expr::Field(f) if matches!(&f.member, Member::Named(name) if name == field) && path(&f.base, "self"))
        }
        _ => false,
    }
}
#[derive(Default)]
struct Constructor {
    allocations: usize,
    inserts: usize,
    collections: usize,
    field_initializations: usize,
    field: String,
}
impl<'ast> Visit<'ast> for Constructor {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Expr::Path(p) = &*call.func {
            let parts: Vec<_> = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            if matches!(parts.as_slice(), [a,b] if a == "BTreeMap" && ["new","default","from","from_iter"].contains(&b.as_str()))
                || matches!(parts.as_slice(), [a,b,c,d] if a == "std" && b == "collections" && c == "BTreeMap" && ["new","default","from","from_iter"].contains(&d.as_str()))
            {
                self.allocations += 1;
                if parts
                    .last()
                    .is_some_and(|p| p == "from" || p == "from_iter")
                {
                    self.inserts += 1;
                }
            }
        }
        visit::visit_expr_call(self, call);
    }
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if call.method == "or_insert" || call.method == "insert" {
            self.inserts += 1;
        }
        if call.method == "collect" {
            self.collections += 1;
            self.inserts += 1;
        }
        visit::visit_expr_method_call(self, call);
    }
    fn visit_expr_struct(&mut self, value: &'ast syn::ExprStruct) {
        if value.path.is_ident("Self") {
            self.field_initializations += value
                .fields
                .iter()
                .filter(|f| matches!(&f.member, Member::Named(name) if name == &self.field))
                .count();
        }
        visit::visit_expr_struct(self, value);
    }
}
fn index_contract(file: &syn::File, class: &str, lookup: &str) -> bool {
    let structs: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Struct(s) if s.ident == class && production(&s.attrs) => Some(s),
            _ => None,
        })
        .collect();
    if structs.len() != 1 {
        return false;
    }
    let indices: Vec<_> = structs[0]
        .fields
        .iter()
        .filter(|f| standard_btree(file, &f.ty))
        .filter_map(|f| f.ident.as_ref())
        .collect();
    if indices.len() != 1 {
        return false;
    }
    let field = indices[0].to_string();
    let mut new_methods = Vec::new();
    let mut get_methods = Vec::new();
    for item in &file.items {
        if let Item::Impl(implementation) = item {
            if !production(&implementation.attrs)
                || implementation.trait_.is_some()
                || !matches!(&*implementation.self_ty, Type::Path(p) if p.path.is_ident(class))
            {
                continue;
            }
            for item in &implementation.items {
                if let ImplItem::Fn(method) = item {
                    if !production(&method.attrs) {
                        continue;
                    }
                    if method.sig.ident == "new" {
                        new_methods.push(method);
                    }
                    if method.sig.ident == lookup {
                        get_methods.push(method);
                    }
                }
            }
        }
    }
    if new_methods.len() != 1 || get_methods.len() != 1 {
        return false;
    }
    let mut constructor = Constructor {
        field: field.clone(),
        ..Constructor::default()
    };
    constructor.visit_block(&new_methods[0].block);
    let get = get_methods[0];
    let argument = parameter(&get.sig).unwrap_or_default();
    constructor.allocations + constructor.collections == 1
        && constructor.inserts >= 1
        && constructor.field_initializations == 1
        && tail(&get.block).is_some_and(|expr| indexed_lookup(expr, &field, &argument))
}
fn check(kind: &str, source: &str) -> Result<bool, String> {
    let file = syn::parse_file(source).map_err(|error| error.to_string())?;
    match kind {
        "catalog_index" => Ok(index_contract(&file, "Catalog", "lookup")),
        "lookup_index" => Ok(index_contract(&file, "Lookup", "get")),
        "json_helper" => Ok(helper_contract(
            &file,
            "encode_json",
            ["name_json", "label_json"],
        )),
        "csv_helper" => Ok(helper_contract(
            &file,
            "encode_csv",
            ["csv_cell", "csv_header"],
        )),
        _ => Err("unsupported syntax contract".into()),
    }
}
fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().collect();
    let result = (|| {
        if args.len() != 3 {
            return Err("expected KIND SOURCE".into());
        }
        let file = File::open(&args[2]).map_err(|_| "source unavailable")?;
        if !file
            .metadata()
            .map_err(|_| "source metadata unavailable")?
            .is_file()
        {
            return Err("source must be a regular file".into());
        }
        let mut source = String::new();
        file.take(2 * 1024 * 1024 + 1)
            .read_to_string(&mut source)
            .map_err(|_| "source read failed")?;
        if source.len() > 2 * 1024 * 1024 {
            return Err("source exceeds limit".into());
        }
        check(&args[1], &source)
    })();
    let (status, code, error) = match result {
        Ok(true) => ("PASS", 0, None),
        Ok(false) => ("FAIL", 1, None),
        Err(error) => ("ERROR", 2, Some(error)),
    };
    println!(
        "{}",
        json!({"status":status,"error":error,"scope":"syntax_contract_plus_separate_behavior_oracle"})
    );
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::check;
    #[test]
    fn wrappers_call_one_private_helper() {
        let valid = "fn encode_json(s:&str)->String{s.into()}pub fn name_json(s:&str)->String{encode_json(s)}pub fn label_json(s:&str)->String{encode_json(s)}";
        assert_eq!(check("json_helper", valid), Ok(true));
        assert_eq!(
            check(
                "json_helper",
                &valid.replace("{encode_json(s)}", "{s.into()}")
            ),
            Ok(false)
        );
        assert_eq!(
            check(
                "json_helper",
                &valid.replace("fn encode_json", "pub fn encode_json")
            ),
            Ok(false)
        );
        assert_eq!(check("json_helper", "// fn encode_json(s:&str)->String{}\npub fn name_json(s:&str)->String{s.into()}pub fn label_json(s:&str)->String{s.into()}"), Ok(false));
    }
    #[test]
    fn index_is_constructed_once_and_lookup_cannot_scan_or_rebuild() {
        let valid = "pub struct Catalog{index:std::collections::BTreeMap<String,i32>}impl Catalog{pub fn new(rows:&[(String,i32)])->Self{let mut index=std::collections::BTreeMap::new();for(k,v)in rows{index.entry(k.clone()).or_insert(*v);}Self{index}}pub fn lookup(&self,key:&str)->Option<i32>{self.index.get(key).copied()}}";
        assert_eq!(check("catalog_index", valid), Ok(true));
        assert_eq!(
            check(
                "catalog_index",
                &valid.replace(
                    "self.index.get(key).copied()",
                    "self.index.iter().find(|(k,_)|k.as_str()==key).map(|(_,v)|*v)"
                )
            ),
            Ok(false)
        );
        assert_eq!(
            check(
                "catalog_index",
                &valid.replace(
                    "self.index.get(key).copied()",
                    "let rebuilt=self.index.clone();rebuilt.get(key).copied()"
                )
            ),
            Ok(false)
        );
        assert_eq!(
            check(
                "catalog_index",
                &valid.replace(
                    "std::collections::BTreeMap::new()",
                    "std::collections::BTreeMap::default()"
                )
            ),
            Ok(true)
        );
        let collected = "pub struct Catalog{index:std::collections::BTreeMap<String,i32>}impl Catalog{pub fn new(rows:&[(String,i32)])->Self{let index=rows.iter().rev().cloned().collect();Self{index}}pub fn lookup(&self,key:&str)->Option<i32>{self.index.get(key).copied()}}";
        assert_eq!(check("catalog_index", collected), Ok(true));
    }
}
