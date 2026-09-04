use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use scah_query_ir::{
    AttributeCaseSensitivity, AttributeSelectionKind, Combinator, ElementPredicate,
    LocalLogicalPredicate, Query, QueryBuilder, QuerySection, Save, SelectionKind,
    StructuralPredicate, Transition,
};
use syn::parse::{Parse, ParseStream};
use syn::{Expr, LitByte, LitStr, Result, Token, braced, bracketed, parenthesized};

/// Generate exact two-nibble SIMD classification tables at compile time.
///
/// The expansion is `([u8; 16], [u8; 16], class_0_bits, class_1_bits, ...)`.
#[proc_macro]
pub fn simd_nibble_tables(input: TokenStream) -> TokenStream {
    let parsed = syn::parse_macro_input!(input as SimdTableInput);
    match generate_simd_mapping(&parsed.classes) {
        Some(mapping) => {
            let tlo = mapping.tlo;
            let thi = mapping.thi;
            let class_bits = mapping.class_bits;
            quote! {
                ([#(#tlo),*], [#(#thi),*], #(#class_bits),*)
            }
            .into()
        }
        None => syn::Error::new(
            Span::call_site(),
            "SIMD nibble classifier is unsatisfiable for these byte domains",
        )
        .to_compile_error()
        .into(),
    }
}

struct SimdTableInput {
    classes: Vec<SimdClassInput>,
}

struct SimdClassInput {
    name: syn::Ident,
    bytes: Vec<u8>,
}

impl Parse for SimdTableInput {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut classes = Vec::new();
        while !input.is_empty() {
            let name: syn::Ident = input.parse()?;
            if classes
                .iter()
                .any(|class: &SimdClassInput| class.name == name)
            {
                return Err(syn::Error::new(name.span(), "duplicate SIMD class name"));
            }
            input.parse::<Token![:]>()?;
            let bytes = parse_byte_array(input)?;
            if bytes.is_empty() {
                return Err(syn::Error::new(name.span(), "SIMD class cannot be empty"));
            }
            classes.push(SimdClassInput { name, bytes });
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        if classes.is_empty() {
            return Err(input.error("expected at least one SIMD class"));
        }
        if classes.len() > 8 {
            return Err(input.error("two-nibble SIMD mappings support at most eight classes"));
        }

        Ok(Self { classes })
    }
}

fn parse_byte_array(input: ParseStream<'_>) -> Result<Vec<u8>> {
    let content;
    bracketed!(content in input);
    let values = content.parse_terminated(LitByte::parse, Token![,])?;
    Ok(values.into_iter().map(|value| value.value()).collect())
}

struct SimdMapping {
    tlo: [u8; 16],
    thi: [u8; 16],
    class_bits: Vec<u8>,
}

fn generate_simd_mapping(classes: &[SimdClassInput]) -> Option<SimdMapping> {
    let target_count = classes.iter().map(|class| class.bytes.len()).sum();
    let mut targets = Vec::with_capacity(target_count);
    let mut target_classes = Vec::with_capacity(target_count);
    for (class_index, class) in classes.iter().enumerate() {
        for &byte in &class.bytes {
            if targets.contains(&byte) {
                return None;
            }
            targets.push(byte);
            target_classes.push(class_index);
        }
    }

    let mut assigned = vec![0; targets.len()];
    let mut tlo = [0; 16];
    let mut thi = [0; 16];
    let mut class_bits = vec![0; classes.len()];
    if !solve_simd_mapping(
        0,
        &targets,
        &target_classes,
        &mut assigned,
        &mut tlo,
        &mut thi,
        &mut class_bits,
    ) {
        return None;
    }

    Some(SimdMapping {
        tlo,
        thi,
        class_bits,
    })
}

#[allow(clippy::too_many_arguments)]
fn solve_simd_mapping(
    index: usize,
    targets: &[u8],
    target_classes: &[usize],
    assigned: &mut [u8],
    tlo: &mut [u8; 16],
    thi: &mut [u8; 16],
    class_bits: &mut [u8],
) -> bool {
    if index == targets.len() {
        return true;
    }

    let byte = targets[index];
    let lo = (byte & 0x0f) as usize;
    let hi = (byte >> 4) as usize;
    let class_index = target_classes[index];

    for shift in 0..8 {
        let mask = 1 << shift;
        if class_bits
            .iter()
            .enumerate()
            .any(|(index, bits)| index != class_index && bits & mask != 0)
        {
            continue;
        }

        let old_lo = tlo[lo];
        let old_hi = thi[hi];
        tlo[lo] |= mask;
        thi[hi] |= mask;
        assigned[index] = mask;
        let old_class_bits = class_bits[class_index];
        class_bits[class_index] |= mask;

        let mut valid = true;
        for candidate in u8::MIN..=u8::MAX {
            let value = tlo[(candidate & 0x0f) as usize] & thi[(candidate >> 4) as usize];
            if value == 0 {
                continue;
            }

            if let Some(target_index) = targets.iter().position(|target| *target == candidate) {
                if target_index <= index {
                    valid &= value == assigned[target_index];
                } else {
                    valid &= value.is_power_of_two();
                }
            } else {
                valid = false;
            }

            if !valid {
                break;
            }
        }

        if valid
            && solve_simd_mapping(
                index + 1,
                targets,
                target_classes,
                assigned,
                tlo,
                thi,
                class_bits,
            )
        {
            return true;
        }

        tlo[lo] = old_lo;
        thi[hi] = old_hi;
        class_bits[class_index] = old_class_bits;
    }

    false
}

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
    let selector = Box::leak(node.selector.value().into_boxed_str());
    let mut builder = match node.kind {
        SelectionKind::All => Query::all(selector, node.save),
        SelectionKind::First => Query::first(selector, node.save),
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
    let metadata_consts = compiled
        .states
        .iter()
        .enumerate()
        .map(metadata_const_tokens)
        .collect::<Vec<_>>();
    let states = compiled
        .states
        .iter()
        .enumerate()
        .map(|(index, transition)| transition_tokens(index, transition));
    let sections = compiled.queries.iter().map(query_section_tokens);
    let alternatives = compiled.alternatives.iter().map(|ranges| {
        let ranges = ranges.iter().map(|range| {
            let start = range.start.index();
            let end = range.end.index();
            quote! { ::std::ops::Range { start: ::scah::TransitionId(#start), end: ::scah::TransitionId(#end) } }
        });
        quote! { const { &[#(#ranges),*] } }
    });
    let num_states = compiled.states.len();
    let num_sections = compiled.queries.len();
    let exit = option_query_section_id_tokens(compiled.exit_at_section_end);

    Ok(quote! {
        {
            #(#attribute_consts)*
            #(#class_consts)*
            #(#metadata_consts)*
            ::scah::StaticQuery::<#num_states, #num_sections>::new(
                [#(#states),*],
                [#(#sections),*],
                #exit,
                const { &[#(#alternatives),*] },
            )
        }
    })
}

fn metadata_const_tokens(
    (index, transition): (usize, &Transition<'_>),
) -> proc_macro2::TokenStream {
    let ident = syn::Ident::new(
        &format!("__SCAH_ATTRIBUTE_NAMES_{index}"),
        Span::call_site(),
    );
    let attribute_names = transition.metadata().attribute_names();
    quote! {
        const #ident: &[&'static str] = &[#(#attribute_names),*];
    }
}

fn class_const_tokens((index, transition): (usize, &Transition<'_>)) -> proc_macro2::TokenStream {
    let ident = syn::Ident::new(&format!("__SCAH_CLASSES_{index}"), Span::call_site());
    let classes = transition
        .predicate()
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
        .predicate()
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
    let predicate = predicate_tokens(index, transition.predicate());
    let name = option_str_tokens(transition.predicate().name);
    let needs_id = transition.metadata().needs_id();
    let needs_class = transition.metadata().needs_class();
    let local_name_only = transition.metadata().local_name_only();
    let names_ident = syn::Ident::new(
        &format!("__SCAH_ATTRIBUTE_NAMES_{index}"),
        Span::call_site(),
    );
    quote! {
        ::scah::Transition::new_const(
            #guard,
            #predicate,
            ::scah::__private::PredicateMetadata::new_const(
                #name,
                #needs_id,
                #needs_class,
                #local_name_only,
                ::scah::__private::AttributeNames::from_static(#names_ident),
            ),
        )
    }
}

fn predicate_tokens(index: usize, predicate: &ElementPredicate<'_>) -> proc_macro2::TokenStream {
    let name = option_str_tokens(predicate.name);
    let id = option_str_tokens(predicate.id);
    let classes_ident = syn::Ident::new(&format!("__SCAH_CLASSES_{index}"), Span::call_site());
    let attrs_ident = syn::Ident::new(&format!("__SCAH_ATTRS_{index}"), Span::call_site());
    let logical = logical_tokens(&predicate.logical);
    let structural = structural_tokens(&predicate.structural);
    quote! {
        ::scah::ElementPredicate::new_const(
            #name,
            #id,
            ::scah::ClassSelections::from_static(#classes_ident),
            ::scah::AttributeSelections::from_static(#attrs_ident),
            #logical,
            #structural,
        )
    }
}

fn logical_tokens(logical: &scah_query_ir::LogicalPredicates<'_>) -> proc_macro2::TokenStream {
    let predicates = logical.as_slice().iter().map(|predicate| {
        let (variant, selectors) = match predicate {
            LocalLogicalPredicate::Not(selectors) => ("Not", selectors),
            LocalLogicalPredicate::Any(selectors) => ("Any", selectors),
        };
        let selector_tokens = selectors.as_slice().iter().map(inline_predicate_tokens);
        let variant_ident = syn::Ident::new(variant, Span::call_site());
        quote! {
            ::scah::LocalLogicalPredicate::#variant_ident(
                ::scah::LocalSelectorList::from_static(const { &[#(#selector_tokens),*] })
            )
        }
    });
    quote! { ::scah::LogicalPredicates::from_static(const { &[#(#predicates),*] }) }
}

fn inline_predicate_tokens(predicate: &ElementPredicate<'_>) -> proc_macro2::TokenStream {
    let name = option_str_tokens(predicate.name);
    let id = option_str_tokens(predicate.id);
    let classes = predicate.classes.as_slice().iter();
    let attrs = predicate
        .attributes
        .as_slice()
        .iter()
        .map(attribute_selection_tokens);
    let logical = logical_tokens(&predicate.logical);
    let structural = structural_tokens(&predicate.structural);
    quote! {
        const {
            const __SCAH_CLASSES: &[&'static str] = &[#(#classes),*];
            const __SCAH_ATTRS: &[::scah::AttributeSelection<'static>] = &[#(#attrs),*];
            ::scah::ElementPredicate::new_const(
                #name,
                #id,
                ::scah::ClassSelections::from_static(__SCAH_CLASSES),
                ::scah::AttributeSelections::from_static(__SCAH_ATTRS),
                #logical,
                #structural,
            )
        }
    }
}

fn structural_tokens(structural: &scah_query_ir::StructuralPredicates) -> proc_macro2::TokenStream {
    let predicates = structural
        .as_slice()
        .iter()
        .map(|predicate| match predicate {
            StructuralPredicate::Root => quote! { ::scah::StructuralPredicate::Root },
            StructuralPredicate::Scope => quote! { ::scah::StructuralPredicate::Scope },
            StructuralPredicate::FirstChild => quote! { ::scah::StructuralPredicate::FirstChild },
            StructuralPredicate::FirstOfType => quote! { ::scah::StructuralPredicate::FirstOfType },
            StructuralPredicate::NthChild(formula) => {
                let a = formula.a;
                let b = formula.b;
                quote! { ::scah::StructuralPredicate::NthChild(::scah::AnPlusB { a: #a, b: #b }) }
            }
            StructuralPredicate::NthOfType(formula) => {
                let a = formula.a;
                let b = formula.b;
                quote! { ::scah::StructuralPredicate::NthOfType(::scah::AnPlusB { a: #a, b: #b }) }
            }
            StructuralPredicate::NthChildOf(formula, filter) => {
                let a = formula.a;
                let b = formula.b;
                let filter = filter
                    .as_slice()
                    .iter()
                    .map(inline_predicate_tokens)
                    .collect::<Vec<_>>();
                quote! {
                    ::scah::StructuralPredicate::NthChildOf(
                        ::scah::AnPlusB { a: #a, b: #b },
                        ::scah::LocalSelectorList::from_static(const { &[#(#filter),*] }),
                    )
                }
            }
        });
    quote! { ::scah::StructuralPredicates::from_static(const { &[#(#predicates),*] }) }
}

fn attribute_selection_tokens(
    attribute: &scah_query_ir::AttributeSelection<'_>,
) -> proc_macro2::TokenStream {
    let name = attribute.name;
    let value = option_str_tokens(attribute.value);
    let kind = attribute_selection_kind_tokens(&attribute.kind);
    let case_sensitivity = match attribute.case_sensitivity {
        AttributeCaseSensitivity::Default => quote! { ::scah::AttributeCaseSensitivity::Default },
        AttributeCaseSensitivity::AsciiInsensitive => {
            quote! { ::scah::AttributeCaseSensitivity::AsciiInsensitive }
        }
        AttributeCaseSensitivity::Sensitive => {
            quote! { ::scah::AttributeCaseSensitivity::Sensitive }
        }
    };
    quote! {
        ::scah::AttributeSelection::new_const(#name, #value, #kind, #case_sensitivity)
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
