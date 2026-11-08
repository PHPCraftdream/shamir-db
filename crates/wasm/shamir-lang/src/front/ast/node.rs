use crate::api::Span;

use super::{
    Expr, ExprId, Function, FunctionId, ParameterId, Statement, StatementId, Type, TypeFieldId,
    TypeId, ValueFieldId,
};

/// Flat module arenas, with every AST node stored once and linked by IDs.
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub functions: Vec<Function>,
    pub function_ids: Vec<FunctionId>,
    pub parameters: Vec<super::function::Parameter>,
    pub parameter_ids: Vec<ParameterId>,
    pub expressions: Vec<Expr>,
    pub value_fields: Vec<super::ty::ValueField>,
    pub value_field_ids: Vec<ValueFieldId>,
    pub statements: Vec<Statement>,
    pub types: Vec<Type>,
    pub type_fields: Vec<super::ty::TypeField>,
    pub type_field_ids: Vec<TypeFieldId>,
    pub span: Span,
}

impl Module {
    /// Makes an empty AST with all arenas initialized.
    pub fn empty(span: Span) -> Self {
        Self {
            functions: Vec::new(),
            function_ids: Vec::new(),
            parameters: Vec::new(),
            parameter_ids: Vec::new(),
            expressions: Vec::new(),
            value_fields: Vec::new(),
            value_field_ids: Vec::new(),
            statements: Vec::new(),
            types: Vec::new(),
            type_fields: Vec::new(),
            type_field_ids: Vec::new(),
            span,
        }
    }

    /// Counts arena nodes in constant time; the module root is not an arena node.
    pub fn node_count(&self) -> usize {
        self.functions
            .len()
            .saturating_add(self.parameters.len())
            .saturating_add(self.expressions.len())
            .saturating_add(self.value_fields.len())
            .saturating_add(self.statements.len())
            .saturating_add(self.types.len())
            .saturating_add(self.type_fields.len())
    }

    /// Looks up one function without unchecked arena access.
    pub fn function(&self, id: FunctionId) -> Option<&Function> {
        self.functions.get(id.0 as usize)
    }

    /// Looks up one parameter without unchecked arena access.
    pub fn parameter(&self, id: ParameterId) -> Option<&super::function::Parameter> {
        self.parameters.get(id.0 as usize)
    }

    /// Looks up one expression without unchecked arena access.
    pub fn expression(&self, id: ExprId) -> Option<&Expr> {
        self.expressions.get(id.0 as usize)
    }

    /// Looks up one value field without unchecked arena access.
    pub fn value_field(&self, id: ValueFieldId) -> Option<&super::ty::ValueField> {
        self.value_fields.get(id.0 as usize)
    }

    /// Looks up one statement without unchecked arena access.
    pub fn statement(&self, id: StatementId) -> Option<&Statement> {
        self.statements.get(id.0 as usize)
    }

    /// Looks up one type without unchecked arena access.
    pub fn ty(&self, id: TypeId) -> Option<&Type> {
        self.types.get(id.0 as usize)
    }

    /// Looks up one type field without unchecked arena access.
    pub fn type_field(&self, id: TypeFieldId) -> Option<&super::ty::TypeField> {
        self.type_fields.get(id.0 as usize)
    }
}
