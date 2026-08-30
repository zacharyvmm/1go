use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use scah_query_ir::{
    AttributeSelectionKind, Combinator, ElementPredicate, Query, QueryBuilder, QuerySection, Save,
    SelectionKind, Transition,
};
use syn::parse::{Parse, ParseStream};
use syn::{Expr, LitStr, Result, Token, braced, parenthesized};

#[proc_macro]
pub fn query(input: TokenStream) -> TokenStream {
    let parsed = syn::parse_macro_input!(input as QueryDsl);
    match expand_query(&parsed.root) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

struct QueryDsl {
    root: QueryNode,
}

impl Parse for QueryDsl {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        Ok(Self {
            root: input.parse()?,
        })
    }
}

#[derive(Clone)]
struct QueryNode {
    kind: SelectionKind,
    selector: LitStr,
    save: Save,
    children: Vec<QueryNode>,
}

impl Parse for QueryNode {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let kind = parse_kind(input)?;
        let content;
        parenthesized!(content in input);
        let selector = parse_selector_literal(&content)?;
        content.parse::<Token![,]>()?;
        let save_expr: Expr = content.parse()?;
        let save = parse_save_expr(&save_expr)?;
        let children = if input.peek(Token![=>]) {
            input.parse::<Token![=>]>()?;
            let children_content;
            braced!(children_content in input);
            let mut children = Vec::new();
            while !children_content.is_empty() {
                children.push(children_content.parse()?);
                if children_content.is_empty() {
                    break;
                }
                children_content.parse::<Token![,]>()?;
            }
            children
        } else {
            Vec::new()
        };

        Ok(Self {
            kind,
            selector,
            save,
            children,
        })
    }
}

fn parse_selector_literal(input: ParseStream<'_>) -> Result<LitStr> {
    if input.peek(LitStr) {
        return input.parse();
    }

    let expr: Expr = input.parse()?;
    Err(syn::Error::new_spanned(
        expr,
        "query! selector must be a string literal, for example `\"a[href]\"`; constants like `QUERY` are not resolved by this macro, so use `Query::all(QUERY, ...)` for shared selector constants",
    ))
}

fn parse_kind(input: ParseStream<'_>) -> Result<SelectionKind> {
    let ident: syn::Ident = input.parse()?;
    match ident.to_string().as_str() {
        "all" => Ok(SelectionKind::All),
        "first" => Ok(SelectionKind::First),
        _ => Err(syn::Error::new(ident.span(), "expected `all` or `first`")),
    }
}

fn parse_save_expr(expr: &Expr) -> Result<Save> {
    if let Expr::MethodCall(call) = expr {
        if call.method == "without_attributes" && call.args.is_empty() {
            return Ok(parse_save_expr(&call.receiver)?.without_attributes());
        }
        return Err(syn::Error::new_spanned(
            expr,
            "unsupported Save modifier in query!",
        ));
    }

    let Expr::Call(call) = expr else {
        return Err(syn::Error::new_spanned(
            expr,
            "expected a supported Save constructor",
        ));
    };
    if !call.args.is_empty() {
        return Err(syn::Error::new_spanned(
            &call.args,
            "save constructors in query! must not take arguments",
        ));
    }

    let Expr::Path(path) = call.func.as_ref() else {
        return Err(syn::Error::new_spanned(
            &call.func,
            "unsupported save expression in query!",
        ));
    };

    let segments: Vec<_> = path
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    match segments.as_slice() {
        [save, method] => match [save.as_str(), method.as_str()] {
            ["Save", "all"] => Ok(Save::all()),
            ["Save", "none"] => Ok(Save::none()),
            ["Save", "only_inner_html"] => Ok(Save::only_inner_html()),
            ["Save", "only_text_content"] => Ok(Save::only_text_content()),
            ["Save", "name_only"] => Ok(Save::name_only()),
            _ => Err(syn::Error::new_spanned(
                expr,
                "unsupported save expression in query!",
            )),
        },
        _ => Err(syn::Error::new_spanned(
            expr,
            "unsupported save expression in query! (too many segments)",
        )),
    }
}

fn compile_node<'a>(node: &'a QueryNode) -> Result<QueryBuilder<'a>> {
    let mut builder = match node.kind {
        SelectionKind::All => {
            Query::all(Box::leak(node.selector.value().into_boxed_str()), node.save)
        }
        SelectionKind::First => {
            Query::first(Box::leak(node.selector.value().into_boxed_str()), node.save)
        }
    }
    .map_err(|err| syn::Error::new(node.selector.span(), err.to_string()))?;

    let current_index = scah_query_ir::QuerySectionId(builder.selection.len() - 1);
    for child in &node.children {
        let child_builder = compile_node(child)?;
        builder.append(current_index, child_builder);
    }

    Ok(builder)
}

fn expand_query(node: &QueryNode) -> Result<proc_macro2::TokenStream> {
    let compiled = compile_node(node)
        .map(QueryBuilder::build)
        .map_err(|err| syn::Error::new(node.selector.span(), err.to_string()))?;

    let attribute_consts = compiled
        .states
        .iter()
        .enumerate()
        .map(attribute_const_tokens)
        .collect::<Vec<_>>();
    let class_consts = compiled
        .states
        .iter()
        .enumerate()
        .map(class_const_tokens)
        .collect::<Vec<_>>();
    let states = compiled
        .states
        .iter()
        .enumerate()
        .map(|(index, transition)| transition_tokens(index, transition));
    let sections = compiled.queries.iter().map(query_section_tokens);
    let num_states = compiled.states.len();
    let num_sections = compiled.queries.len();
    let exit = option_query_section_id_tokens(compiled.exit_at_section_end);

    Ok(quote! {
        {
            #(#attribute_consts)*
            #(#class_consts)*
            ::scah::StaticQuery::<#num_states, #num_sections>::new(
                [#(#states),*],
                [#(#sections),*],
                #exit,
            )
        }
    })
}

fn class_const_tokens((index, transition): (usize, &Transition<'_>)) -> proc_macro2::TokenStream {
    let ident = syn::Ident::new(&format!("__SCAH_CLASSES_{index}"), Span::call_site());
    let classes = transition
        .predicate
        .classes
        .as_slice()
        .iter()
        .map(|class| quote! { #class });
    quote! {
        const #ident: &[&'static str] = &[#(#classes),*];
    }
}

fn attribute_const_tokens(
    (index, transition): (usize, &Transition<'_>),
) -> proc_macro2::TokenStream {
    let ident = syn::Ident::new(&format!("__SCAH_ATTRS_{index}"), Span::call_site());
    let attrs = transition
        .predicate
        .attributes
        .as_slice()
        .iter()
        .map(attribute_selection_tokens);
    quote! {
        const #ident: &[::scah::AttributeSelection<'static>] = &[#(#attrs),*];
    }
}

fn transition_tokens(index: usize, transition: &Transition<'_>) -> proc_macro2::TokenStream {
    let guard = combinator_tokens(&transition.guard);
    let predicate = predicate_tokens(index, &transition.predicate);
    quote! { ::scah::Transition::new_const(#guard, #predicate) }
}

fn predicate_tokens(index: usize, predicate: &ElementPredicate<'_>) -> proc_macro2::TokenStream {
    let name = option_str_tokens(predicate.name);
    let id = option_str_tokens(predicate.id);
    let classes_ident = syn::Ident::new(&format!("__SCAH_CLASSES_{index}"), Span::call_site());
    let attrs_ident = syn::Ident::new(&format!("__SCAH_ATTRS_{index}"), Span::call_site());
    quote! {
        ::scah::ElementPredicate::new_const(
            #name,
            #id,
            ::scah::ClassSelections::from_static(#classes_ident),
            ::scah::AttributeSelections::from_static(#attrs_ident),
        )
    }
}

fn attribute_selection_tokens(
    attribute: &scah_query_ir::AttributeSelection<'_>,
) -> proc_macro2::TokenStream {
    let name = attribute.name;
    let value = option_str_tokens(attribute.value);
    let kind = attribute_selection_kind_tokens(&attribute.kind);
    quote! {
        ::scah::AttributeSelection::new_const(#name, #value, #kind)
    }
}

fn query_section_tokens(section: &QuerySection<'_>) -> proc_macro2::TokenStream {
    let source = section.source;
    let save = save_tokens(section.save);
    let kind = selection_kind_tokens(section.kind);
    let start = section.range.start.index();
    let end = section.range.end.index();
    let parent = option_query_section_id_tokens(section.parent);
    let next_sibling = option_query_section_id_tokens(section.next_sibling);
    quote! {
        ::scah::QuerySection::new_const(
            #source,
            #save,
            #kind,
            ::scah::TransitionId(#start)..::scah::TransitionId(#end),
            #parent,
            #next_sibling,
        )
    }
}

fn save_tokens(save: Save) -> proc_macro2::TokenStream {
    let inner_html = save.inner_html;
    let text_content = save.text_content;
    let attributes = save.attributes;
    quote! { ::scah::Save { inner_html: #inner_html, text_content: #text_content, attributes: #attributes } }
}

fn selection_kind_tokens(kind: SelectionKind) -> proc_macro2::TokenStream {
    match kind {
        SelectionKind::All => quote! { ::scah::SelectionKind::All },
        SelectionKind::First => quote! { ::scah::SelectionKind::First },
    }
}

fn combinator_tokens(kind: &Combinator) -> proc_macro2::TokenStream {
    match kind {
        Combinator::Child => quote! { ::scah::Combinator::Child },
        Combinator::Descendant => quote! { ::scah::Combinator::Descendant },
        Combinator::NextSibling => quote! { ::scah::Combinator::NextSibling },
        Combinator::SubsequentSibling => quote! { ::scah::Combinator::SubsequentSibling },
        Combinator::Namespace => quote! { ::scah::Combinator::Namespace },
    }
}

fn attribute_selection_kind_tokens(kind: &AttributeSelectionKind) -> proc_macro2::TokenStream {
    match kind {
        AttributeSelectionKind::Exact => quote! { ::scah::AttributeSelectionKind::Exact },
        AttributeSelectionKind::Prefix => quote! { ::scah::AttributeSelectionKind::Prefix },
        AttributeSelectionKind::Suffix => quote! { ::scah::AttributeSelectionKind::Suffix },
        AttributeSelectionKind::Substring => quote! { ::scah::AttributeSelectionKind::Substring },
        AttributeSelectionKind::Presence => quote! { ::scah::AttributeSelectionKind::Presence },
        AttributeSelectionKind::WhitespaceSeparated => {
            quote! { ::scah::AttributeSelectionKind::WhitespaceSeparated }
        }
        AttributeSelectionKind::HyphenSeparated => {
            quote! { ::scah::AttributeSelectionKind::HyphenSeparated }
        }
    }
}

fn option_str_tokens(value: Option<&str>) -> proc_macro2::TokenStream {
    match value {
        Some(value) => quote! { Some(#value) },
        None => quote! { None },
    }
}

fn option_query_section_id_tokens(
    value: Option<scah_query_ir::QuerySectionId>,
) -> proc_macro2::TokenStream {
    match value {
        Some(value) => {
            let index = value.index();
            quote! { Some(::scah::QuerySectionId(#index)) }
        }
        None => quote! { None },
    }
}

#[cfg(test)]
mod tests {
    use super::{QueryNode, parse_save_expr};
    use syn::Expr;

    #[test]
    fn rejects_selector_constants_with_actionable_error() {
        let error = match syn::parse_str::<QueryNode>("all(QUERY, Save::all())") {
            Ok(_) => panic!("selector constant should not parse"),
            Err(error) => error,
        };

        let message = error.to_string();
        assert!(message.contains("selector must be a string literal"));
        assert!(message.contains("constants like `QUERY` are not resolved by this macro"));
        assert!(message.contains("Query::all(QUERY, ...)"));
    }

    #[test]
    fn accepts_name_only_save_constructor() {
        let expression = syn::parse_str::<Expr>("Save::name_only()").unwrap();
        assert_eq!(
            parse_save_expr(&expression).unwrap(),
            scah_query_ir::Save::name_only()
        );
    }

    #[test]
    fn accepts_without_attributes_save_modifier() {
        let expression =
            syn::parse_str::<Expr>("Save::only_text_content().without_attributes()").unwrap();
        assert_eq!(
            parse_save_expr(&expression).unwrap(),
            scah_query_ir::Save::only_text_content().without_attributes()
        );
    }
}
