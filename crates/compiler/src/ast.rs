use crate::{FileId, SourceRange};

#[derive(Clone, Debug)]
pub struct Module {
    pub file_id: FileId,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
    pub declarations: Vec<Declaration>,
    pub native_blocks: Vec<NativeBlock>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct Export {
    pub path: Vec<String>,
    /// Synthesized from an exported owner type, never written as a method export.
    pub implicit: bool,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct Import {
    /// Source-visible qualifier: dotted segments, or one explicit file alias.
    pub path: Vec<String>,
    /// Present only for a file literal; `path` is its alias, not a disk path.
    pub file: Option<FileImport>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct FileImport {
    pub path: String,
    pub path_range: SourceRange,
    pub alias_range: SourceRange,
}

#[derive(Clone, Debug)]
pub enum Declaration {
    Function(FunctionDecl),
    Record(RecordDecl),
    Enum(EnumDecl),
    Error(ErrorDecl),
    Constant(ConstantDecl),
    TypeAlias(TypeAliasDecl),
}

impl Declaration {
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Function(value) => &value.name,
            Self::Record(value) => &value.name,
            Self::Enum(value) => &value.name,
            Self::Error(value) => &value.name,
            Self::Constant(value) => &value.name,
            Self::TypeAlias(value) => &value.name,
        }
    }

    #[must_use]
    pub const fn range(&self) -> SourceRange {
        match self {
            Self::Function(value) => value.range,
            Self::Record(value) => value.range,
            Self::Enum(value) => value.range,
            Self::Error(value) => value.range,
            Self::Constant(value) => value.range,
            Self::TypeAlias(value) => value.range,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionDecl {
    pub name: String,
    pub is_private: bool,
    /// Canonical lexical owner; inherited by closures created inside a method.
    pub lexical_owner: Option<String>,
    pub type_parameters: Vec<TypeParameter>,
    pub is_async: bool,
    pub parameters: Vec<Parameter>,
    pub result: TypeRef,
    pub body: Block,
    pub fallback: Option<Expression>,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub ty: TypeRef,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct RecordDecl {
    pub mutable: bool,
    pub name: String,
    pub type_parameters: Vec<TypeParameter>,
    pub fields: Vec<FieldDecl>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub name: String,
    pub type_parameters: Vec<TypeParameter>,
    pub variants: Vec<EnumVariant>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct ErrorDecl {
    pub name: String,
    pub type_parameters: Vec<TypeParameter>,
    pub payload: Option<ErrorPayloadDecl>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct ErrorPayloadDecl {
    pub optional: bool,
    pub ty: TypeRef,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct EnumVariant {
    pub name: String,
    pub fields: Vec<FieldDecl>,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDecl {
    pub name: String,
    /// Immutable after aggregate construction. Methods are not fields.
    pub is_const: bool,
    pub ty: TypeRef,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeParameter {
    pub name: String,
    pub constraints: Vec<String>,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeRef {
    pub kind: TypeRefKind,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeRefKind {
    Record(Vec<FieldDecl>),
    Named(Vec<String>),
    Generic {
        path: Vec<String>,
        arguments: Vec<TypeRef>,
    },
    List(Box<TypeRef>),
    Map(Box<TypeRef>, Box<TypeRef>),
    Future(Box<TypeRef>),
    Bytes(Box<Expression>),
    /// Resolved generic length; does not change ordinary size-expression typing.
    ConcreteBytes(u64),
    Function {
        supplant: bool,
        parameters: Vec<TypeRef>,
        result: Box<TypeRef>,
    },
}

impl TypeRef {
    #[must_use]
    pub fn unit(range: SourceRange) -> Self {
        Self {
            kind: TypeRefKind::Named(vec!["Unit".to_owned()]),
            range,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub range: SourceRange,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AllCatch {
    pub binding: String,
    pub binding_range: SourceRange,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatchBinding {
    pub name: String,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatchElement {
    pub binding: Option<CatchBinding>,
    pub ty: TypeRef,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CatchPattern {
    Binding(CatchBinding),
    List {
        elements: Vec<CatchElement>,
        rest: Option<CatchBinding>,
        range: SourceRange,
    },
}

impl CatchPattern {
    pub fn bindings(&self) -> impl Iterator<Item = &CatchBinding> {
        let (single, elements, rest): (Option<&CatchBinding>, &[CatchElement], _) = match self {
            Self::Binding(binding) => (Some(binding), &[], None),
            Self::List { elements, rest, .. } => (None, elements, rest.as_ref()),
        };
        single
            .into_iter()
            .chain(
                elements
                    .iter()
                    .filter_map(|element| element.binding.as_ref()),
            )
            .chain(rest)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatchClause {
    pub pattern: CatchPattern,
    pub body: Block,
    pub range: SourceRange,
}

impl CatchClause {
    pub(crate) fn whole(binding: &str, range: SourceRange, body: &Block) -> Self {
        Self {
            pattern: CatchPattern::Binding(CatchBinding {
                name: binding.into(),
                range,
            }),
            body: body.clone(),
            range: range.join(body.range),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatementKind {
    Throw(Expression),
    /// Ordered whole-list catches, including typed element and rest patterns.
    TryErrors {
        all: bool,
        body: Block,
        handlers: Vec<CatchClause>,
        cleanup: Option<Block>,
    },
    TryAll {
        body: Block,
        handler: Option<AllCatch>,
        cleanup: Option<Block>,
    },
    Block(Block),
    Local {
        mutable: bool,
        name: String,
        annotation: Option<TypeRef>,
        initializer: Expression,
    },
    FieldAssign {
        base: Expression,
        field: String,
        value: Expression,
    },
    Assign {
        name: String,
        value: Expression,
    },
    Expression(Expression),
    Return(Option<Expression>),
    Break,
    Continue,
    If {
        condition: Expression,
        then_block: Block,
        else_branch: Option<Box<Statement>>,
    },
    While {
        condition: Expression,
        body: Block,
    },
    Switch {
        value: Expression,
        cases: Vec<SwitchCase>,
    },
    TryCatch {
        body: Block,
        binding: String,
        binding_range: SourceRange,
        handler: Block,
        cleanup: Option<Block>,
    },
    TryFinally {
        body: Block,
        cleanup: Block,
    },
    TaskScope {
        body: Block,
        cleanup: Block,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwitchCase {
    pub path: Vec<String>,
    pub bindings: Vec<String>,
    pub body: Block,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExpressionKind {
    Function(Box<FunctionDecl>),
    Record(Vec<RecordField>),
    RecordUpdate {
        base: Box<Expression>,
        fields: Vec<(String, Expression, SourceRange)>,
    },
    Literal(Literal),
    FormatString(Vec<Expression>),
    Name(Vec<String>),
    Unit,
    Call {
        target: Vec<String>,
        arguments: Arguments,
    },
    Bind {
        target: Vec<String>,
        type_arguments: Vec<TypeRef>,
        arguments: Arguments,
    },
    GenericCall {
        target: Vec<String>,
        type_arguments: Vec<TypeRef>,
        variant: Option<String>,
        arguments: Arguments,
        spawn: bool,
    },
    List(Vec<Expression>),
    Map(Vec<(Expression, Expression)>),
    Unary {
        operator: UnaryOperator,
        operand: Box<Expression>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    Is {
        value: Box<Expression>,
        ty: TypeRef,
    },
    Await(Box<Expression>),
    Spawn {
        target: Vec<String>,
        arguments: Arguments,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordField {
    pub name: String,
    pub annotation: Option<TypeRef>,
    pub value: Expression,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct TypeAliasDecl {
    pub name: String,
    pub type_parameters: Vec<TypeParameter>,
    pub ty: TypeRef,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arguments {
    Positional(Vec<Expression>),
    Named(Vec<(String, Expression, SourceRange)>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    Unsigned(u64),
    DecimalInteger(String),
    Float { value: f64, spelling: String },
    String(String),
    Bytes(Vec<u8>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOperator {
    Not,
    Negate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperator {
    Or,
    And,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

#[derive(Clone, Debug)]
pub struct ConstantDecl {
    pub name: String,
    pub annotation: Option<TypeRef>,
    pub initializer: Expression,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct NativeBlock {
    pub members: Vec<NativeMember>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub enum NativeMember {
    Opaque { name: String, range: SourceRange },
    Record(RecordDecl),
    Function(NativeFunctionDecl),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeBinding {
    Symbol(String),
    /// Created by the compiler for standard-library calls, never parsed.
    Runtime(crate::RuntimeFunction),
}

#[derive(Clone, Debug)]
pub struct NativeFunctionDecl {
    pub name: String,
    pub is_async: bool,
    pub is_supplant: bool,
    pub parameters: Vec<Parameter>,
    pub result: TypeRef,
    pub binding: NativeBinding,
    pub binding_range: SourceRange,
    pub range: SourceRange,
}
