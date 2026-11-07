//! Public source templates are metadata; only concrete instances enter typed IR.
use crate::ast::{
    Arguments, Declaration, Expression, ExpressionKind, FieldDecl, Literal, TypeParameter, TypeRef,
    TypeRefKind,
};

#[derive(Clone, Debug)]
pub struct SourceTemplateSchema {
    pub kind: String,
    pub type_parameters: Vec<(String, Vec<String>)>,
    pub owner_type_parameters: Vec<(String, Vec<String>)>,
    pub interface_constraints: std::collections::BTreeMap<String, Vec<String>>,
    pub interfaces: Vec<String>,
    /// None means inference; Some([]) is an explicit nothrow contract.
    pub error_contract: Option<Vec<String>>,
    pub methods: Vec<(String, String)>,
    pub signature: String,
    pub const_fields: Vec<u32>,
    pub private_fields: Vec<u32>,
    pub fields: Vec<(String, String)>,
    pub variants: Vec<(String, Vec<(String, String)>)>,
}

fn fields(values: &[FieldDecl]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|field| (field.name.clone(), ty(&field.ty)))
        .collect()
}
fn type_parameters(values: &[TypeParameter]) -> Vec<(String, Vec<String>)> {
    values
        .iter()
        .map(|parameter| {
            let mut markers = parameter.constraints.clone();
            markers.sort();
            markers.dedup();
            (parameter.name.clone(), markers)
        })
        .collect()
}

fn has_callback_contract(ty: &TypeRef) -> bool {
    match &ty.kind {
        TypeRefKind::Function {
            parameters,
            result,
            error_contract,
            ..
        } => {
            error_contract.is_some()
                || parameters.iter().any(has_callback_contract)
                || has_callback_contract(result)
        }
        TypeRefKind::Generic { arguments, .. } => arguments.iter().any(has_callback_contract),
        TypeRefKind::Record(fields) => fields.iter().any(|field| has_callback_contract(&field.ty)),
        TypeRefKind::List(ty) | TypeRefKind::Future(ty) => has_callback_contract(ty),
        TypeRefKind::Map(key, value) => has_callback_contract(key) || has_callback_contract(value),
        _ => false,
    }
}

pub(crate) fn schema(declaration: &Declaration) -> Option<SourceTemplateSchema> {
    let mut interfaces = Vec::new();
    let mut error_contract = None;
    let mut methods = Vec::new();
    let callback_contract = match declaration {
        Declaration::Function(function) => {
            function
                .parameters
                .iter()
                .any(|parameter| has_callback_contract(&parameter.ty))
                || has_callback_contract(&function.result)
        }
        Declaration::Record(record) => record
            .fields
            .iter()
            .any(|field| has_callback_contract(&field.ty)),
        Declaration::Enum(enumeration) => enumeration
            .variants
            .iter()
            .flat_map(|variant| &variant.fields)
            .any(|field| has_callback_contract(&field.ty)),
        Declaration::Error(error) => error
            .payload
            .as_ref()
            .is_some_and(|payload| has_callback_contract(&payload.ty)),
        _ => false,
    };
    let (parameters, kind, signature, members, variants) = match declaration {
        Declaration::Function(function) => {
            error_contract = function
                .error_contract
                .as_ref()
                .map(|c| canonical_types(&c.types));
            (
                &function.type_parameters,
                "function",
                signature(function),
                vec![],
                vec![],
            )
        }
        Declaration::Interface(interface) => {
            methods = interface
                .methods
                .iter()
                .map(|m| (m.name.clone(), signature(m)))
                .collect();
            methods.sort();
            (
                &interface.type_parameters,
                "interface",
                String::new(),
                vec![],
                vec![],
            )
        }
        Declaration::Record(record) => {
            interfaces = canonical_types(&record.interfaces);
            (
                &record.type_parameters,
                "class",
                String::new(),
                record.fields.clone(),
                vec![],
            )
        }
        Declaration::Enum(enumeration) => {
            interfaces = canonical_types(&enumeration.interfaces);
            (
                &enumeration.type_parameters,
                "enum",
                String::new(),
                vec![],
                enumeration
                    .variants
                    .iter()
                    .map(|v| (v.name.clone(), fields(&v.fields)))
                    .collect(),
            )
        }
        Declaration::TypeAlias(alias) => (
            &alias.type_parameters,
            "type-alias",
            ty(&alias.ty),
            vec![],
            vec![],
        ),
        Declaration::Error(error) => {
            if error.type_parameters.is_empty() && !callback_contract {
                return None;
            }
            let mut fields = vec![
                ("name".into(), "String".into()),
                ("message".into(), "String".into()),
                ("source".into(), "Option<$rinzu_SourceLocation>".into()),
                ("cause".into(), "Option<Error>".into()),
            ];
            let signature = if let Some(payload) = &error.payload {
                let input = ty(&payload.ty);
                fields.push((
                    "payload".into(),
                    if payload.optional {
                        format!("Option<{input}>")
                    } else {
                        input.clone()
                    },
                ));
                format!(
                    "error(payload{}:{input})",
                    if payload.optional { "?" } else { "" }
                )
            } else {
                "error()".into()
            };
            return Some(SourceTemplateSchema {
                owner_type_parameters: Vec::new(),
                interface_constraints: interface_constraints(&error.type_parameters),
                interfaces: vec![],
                error_contract: None,
                methods: vec![],
                private_fields: Vec::new(),
                kind: "error".into(),
                type_parameters: type_parameters(&error.type_parameters),
                signature,
                const_fields: (0..fields.len() as u32).collect(),
                fields,
                variants: vec![],
            });
        }
        _ => return None,
    };
    if parameters.is_empty()
        && kind != "interface"
        && kind != "type-alias"
        && interfaces.is_empty()
        && error_contract.is_none()
        && !callback_contract
    {
        return None;
    }
    let owner_count = if let Declaration::Function(f) = declaration {
        f.owner_type_parameter_count
    } else {
        0
    };
    Some(SourceTemplateSchema {
        owner_type_parameters: type_parameters(&parameters[..owner_count]),
        interface_constraints: interface_constraints(parameters),
        interfaces,
        error_contract,
        methods,
        private_fields: members
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_private)
            .map(|(i, _)| i as u32)
            .collect(),
        kind: kind.into(),
        type_parameters: type_parameters(&parameters[owner_count..]),
        signature,
        const_fields: members
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_const)
            .map(|(i, _)| i as u32)
            .collect(),
        fields: fields(&members),
        variants,
    })
}

fn canonical_types(values: &[TypeRef]) -> Vec<String> {
    let mut values: Vec<_> = values.iter().map(ty).collect();
    values.sort();
    values.dedup();
    values
}

fn interface_constraints(
    parameters: &[TypeParameter],
) -> std::collections::BTreeMap<String, Vec<String>> {
    parameters
        .iter()
        .filter(|p| !p.interfaces.is_empty())
        .map(|p| (p.name.clone(), canonical_types(&p.interfaces)))
        .collect()
}

fn signature(f: &crate::ast::FunctionDecl) -> String {
    format!(
        "{}({}){}->{}{}",
        if f.is_async { "async" } else { "fn" },
        f.parameters
            .iter()
            .map(|p| ty(&p.ty))
            .collect::<Vec<_>>()
            .join(","),
        f.error_contract
            .as_ref()
            .map_or_else(String::new, |contract| contract.spelling(ty)),
        ty(&f.result),
        f.fallback
            .as_ref()
            .map_or_else(String::new, |e| format!(" fallback {}", expression(e)))
    )
}

fn ty(value: &TypeRef) -> String {
    match &value.kind {
        TypeRefKind::Named(path) => path.join("."),
        TypeRefKind::Generic { path, arguments } => format!(
            "{}<{}>",
            path.join("."),
            arguments.iter().map(ty).collect::<Vec<_>>().join(",")
        ),
        TypeRefKind::Record(values) => {
            let mut values = fields(values)
                .into_iter()
                .map(|(name, ty)| format!("{name}:{ty}"))
                .collect::<Vec<_>>();
            values.sort();
            format!("{{{}}}", values.join(","))
        }
        TypeRefKind::List(value) => format!("List<{}>", ty(value)),
        TypeRefKind::Map(key, value) => format!("Map<{},{}>", ty(key), ty(value)),
        TypeRefKind::Future(value) => format!("Future<{}>", ty(value)),
        TypeRefKind::ConcreteBytes(value) => format!("Bytes({value})"),
        TypeRefKind::Bytes(value) => format!("Bytes({})", expression(value)),
        TypeRefKind::Function {
            supplant,
            parameters,
            result,
            error_contract,
        } => format!(
            "{}fn({}){}->{}",
            if *supplant { "supplant " } else { "" },
            parameters.iter().map(ty).collect::<Vec<_>>().join(","),
            error_contract
                .as_ref()
                .map_or_else(String::new, |contract| contract.spelling(ty)),
            ty(result)
        ),
    }
}
fn expression(value: &Expression) -> String {
    fn args(values: &Arguments) -> String {
        match values {
            Arguments::Positional(values) => {
                values.iter().map(expression).collect::<Vec<_>>().join(",")
            }
            Arguments::Named(values) => values
                .iter()
                .map(|(name, value, _)| format!("{name}:{}", expression(value)))
                .collect::<Vec<_>>()
                .join(","),
        }
    }
    match &value.kind {
        ExpressionKind::Literal(Literal::Int(value)) => value.to_string(),
        ExpressionKind::Literal(Literal::Unsigned(value)) => value.to_string(),
        ExpressionKind::Literal(Literal::DecimalInteger(value)) => value.clone(),
        ExpressionKind::Literal(Literal::Bool(value)) => value.to_string(),
        ExpressionKind::Literal(Literal::Float { spelling, .. }) => spelling.clone(),
        ExpressionKind::Literal(Literal::String(value)) => format!("{value:?}"),
        ExpressionKind::Literal(Literal::Bytes(value)) => format!("bytes:{value:02x?}"),
        ExpressionKind::Unit => "()".into(),
        ExpressionKind::Name(path) => path.join("."),
        ExpressionKind::Unary { operator, operand } => {
            format!("({operator:?}{})", expression(operand))
        }
        ExpressionKind::Binary {
            operator,
            left,
            right,
        } => format!("({}{operator:?}{})", expression(left), expression(right)),
        ExpressionKind::Call { target, arguments } => {
            format!("{}({})", target.join("."), args(arguments))
        }
        ExpressionKind::GenericCall {
            target,
            type_arguments,
            arguments,
            ..
        } => format!(
            "{}<{}>({})",
            target.join("."),
            type_arguments.iter().map(ty).collect::<Vec<_>>().join(","),
            args(arguments)
        ),
        // Invalid size expressions are rejected at concrete specialization;
        // never put diagnostic ranges/addresses into interface identity.
        _ => "invalid-size-expression".into(),
    }
}
