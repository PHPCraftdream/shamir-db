use super::body::{Encoder, SchemaMigrationCanonicalError};
use crate::admin::types::schema_ops::{FieldRuleDto, NumDto};
use crate::admin::types::{
    AddSchemaRuleOp, AlterFunctionOp, BindValidatorOp, CheckFunctionOp, CreateIndexOp,
    CreateTableOp, RemoveSchemaRuleOp, RenameFunctionOp, RenameIndexOp, RenameTableOp,
    RenameValidatorOp, SetTableSchemaOp, UnbindValidatorOp,
};
use crate::filter::FilterValue;
use shamir_types::types::value::QueryValue;

const PREFIX: &[u8] = b"shamir-db schema migration op v1\0";

fn start(tag: u16) -> Encoder {
    let mut encoder = Encoder::new();
    encoder.bytes.extend_from_slice(PREFIX);
    encoder.u16(tag);
    encoder
}

fn finish(encoder: Encoder) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    Ok(encoder.bytes)
}

fn strings(encoder: &mut Encoder, values: &[String]) -> Result<(), SchemaMigrationCanonicalError> {
    encoder.strings(values)
}

fn paths(
    encoder: &mut Encoder,
    values: &[Vec<String>],
) -> Result<(), SchemaMigrationCanonicalError> {
    encoder.count(values.len())?;
    for path in values {
        strings(encoder, path)?;
    }
    Ok(())
}

pub(super) fn encode_create_table(
    value: &CreateTableOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let CreateTableOp {
        create_table,
        repo,
        if_not_exists,
        retention,
        schema,
    } = value;
    let mut encoder = start(1);
    encoder.str(create_table)?;
    encoder.str(repo)?;
    encoder.bool(*if_not_exists);
    encoder.optional(retention.as_ref(), |e, value| {
        e.optional(value.max_age_secs.as_ref(), |e, value| {
            e.u64(*value);
            Ok(())
        })?;
        e.optional(value.max_count.as_ref(), |e, value| {
            e.u64(*value);
            Ok(())
        })?;
        e.optional(value.min_count.as_ref(), |e, value| {
            e.u64(*value);
            Ok(())
        })
    })?;
    encoder.optional(schema.as_ref(), |e, schema| encode_rules(e, schema))?;
    finish(encoder)
}

pub(super) fn encode_rename_table(
    value: &RenameTableOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let RenameTableOp {
        rename_table,
        to,
        repo,
    } = value;
    let mut encoder = start(3);
    encoder.str(rename_table)?;
    encoder.str(to)?;
    encoder.str(repo)?;
    finish(encoder)
}

pub(super) fn encode_create_index(
    value: &CreateIndexOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let CreateIndexOp {
        create_index,
        table,
        fields,
        unique,
        sorted,
        repo,
        index_type,
        fts_tokenizer,
        fts_language,
        functional_op,
        functional_args,
        vector_dim,
        vector_metric,
        vector_quantization,
        include,
        if_not_exists,
    } = value;
    let mut encoder = start(4);
    encoder.str(create_index)?;
    encoder.str(table)?;
    paths(&mut encoder, fields)?;
    encoder.bool(*unique);
    encoder.bool(*sorted);
    encoder.str(repo)?;
    encode_optional_str(&mut encoder, index_type.as_ref())?;
    encode_optional_str(&mut encoder, fts_tokenizer.as_ref())?;
    encode_optional_str(&mut encoder, fts_language.as_ref())?;
    encode_optional_str(&mut encoder, functional_op.as_ref())?;
    encoder.optional(functional_args.as_ref(), |e, args| {
        e.count(args.len())?;
        for value in args {
            encode_value(e, value, 0)?;
        }
        Ok(())
    })?;
    encoder.optional(vector_dim.as_ref(), |e, value| {
        e.u32(*value);
        Ok(())
    })?;
    encode_optional_str(&mut encoder, vector_metric.as_ref())?;
    encode_optional_str(&mut encoder, vector_quantization.as_ref())?;
    paths(&mut encoder, include)?;
    encoder.bool(*if_not_exists);
    finish(encoder)
}

pub(super) fn encode_rename_index(
    value: &RenameIndexOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let RenameIndexOp {
        rename_index,
        to,
        table,
        repo,
        if_exists,
        request_id: _,
    } = value;
    let mut encoder = start(6);
    encoder.str(rename_index)?;
    encoder.str(to)?;
    encoder.str(table)?;
    encoder.str(repo)?;
    encoder.bool(*if_exists);
    finish(encoder)
}

pub(super) fn encode_set_schema(
    value: &SetTableSchemaOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let SetTableSchemaOp {
        set_table_schema,
        repo,
        schema,
        expected_version,
    } = value;
    let mut encoder = start(7);
    encoder.str(set_table_schema)?;
    encoder.str(repo)?;
    encode_rules(&mut encoder, schema)?;
    encoder.optional(expected_version.as_ref(), |e, value| {
        e.u64(*value);
        Ok(())
    })?;
    finish(encoder)
}

pub(super) fn encode_add_schema_rule(
    value: &AddSchemaRuleOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let AddSchemaRuleOp {
        add_schema_rule,
        repo,
        rule,
    } = value;
    let mut encoder = start(8);
    encoder.str(add_schema_rule)?;
    encoder.str(repo)?;
    encode_rule(&mut encoder, rule)?;
    finish(encoder)
}

pub(super) fn encode_remove_schema_rule(
    value: &RemoveSchemaRuleOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let RemoveSchemaRuleOp {
        remove_schema_rule,
        repo,
        path,
    } = value;
    let mut encoder = start(9);
    encoder.str(remove_schema_rule)?;
    encoder.str(repo)?;
    strings(&mut encoder, path)?;
    finish(encoder)
}

pub(super) fn encode_rename_function(
    value: &RenameFunctionOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let RenameFunctionOp {
        rename_function,
        to,
    } = value;
    let mut encoder = start(12);
    encoder.str(rename_function)?;
    encoder.str(to)?;
    finish(encoder)
}

pub(super) fn encode_check_function(
    value: &CheckFunctionOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let CheckFunctionOp { check_function } = value;
    let crate::admin::types::function_ops::CheckFunctionSpec {
        lang,
        wasm,
        source,
        optimize,
    } = check_function;
    let resolved_lang = lang
        .as_deref()
        .unwrap_or(if wasm.is_some() { "wasm" } else { "rust" });
    let resolved_optimize = optimize.as_deref().unwrap_or("auto");
    let mut encoder = start(13);
    encoder.str(resolved_lang)?;
    encode_optional_str(&mut encoder, wasm.as_ref())?;
    encode_optional_str(&mut encoder, source.as_ref())?;
    encoder.str(resolved_optimize)?;
    finish(encoder)
}

pub(super) fn encode_alter_function(
    value: &AlterFunctionOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let AlterFunctionOp {
        alter_function,
        optimize,
    } = value;
    let mut encoder = start(14);
    encoder.str(alter_function)?;
    encoder.str(optimize)?;
    finish(encoder)
}

pub(super) fn encode_rename_validator(
    value: &RenameValidatorOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let RenameValidatorOp {
        rename_validator,
        to,
    } = value;
    let mut encoder = start(17);
    encoder.str(rename_validator)?;
    encoder.str(to)?;
    finish(encoder)
}

pub(super) fn encode_bind_validator(
    value: &BindValidatorOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let BindValidatorOp {
        bind_validator,
        db,
        repo,
        table,
        ops,
        priority,
    } = value;
    let mut encoder = start(18);
    encoder.str(bind_validator)?;
    encoder.str(db)?;
    encoder.str(repo)?;
    encoder.str(table)?;
    encoder.count(ops.len())?;
    for op in ops {
        encoder.u8(match op {
            crate::WriteOp::Insert => 0,
            crate::WriteOp::Update => 1,
            crate::WriteOp::Upsert => 2,
            crate::WriteOp::Delete => 3,
        });
    }
    encoder.u16(*priority);
    finish(encoder)
}

pub(super) fn encode_unbind_validator(
    value: &UnbindValidatorOp,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let UnbindValidatorOp {
        unbind_validator,
        db,
        repo,
        table,
    } = value;
    let mut encoder = start(19);
    encoder.str(unbind_validator)?;
    encoder.str(db)?;
    encoder.str(repo)?;
    encoder.str(table)?;
    finish(encoder)
}

#[cfg(test)]
pub(crate) fn canonical_query_value(
    value: &shamir_types::types::value::QueryValue,
) -> Result<Vec<u8>, SchemaMigrationCanonicalError> {
    let mut encoder = Encoder::new();
    encode_value(&mut encoder, value, 0)?;
    Ok(encoder.bytes)
}

fn encode_optional_str(
    encoder: &mut Encoder,
    value: Option<&String>,
) -> Result<(), SchemaMigrationCanonicalError> {
    encoder.optional(value, |e, value| e.str(value))
}

fn encode_rules(
    encoder: &mut Encoder,
    rules: &[FieldRuleDto],
) -> Result<(), SchemaMigrationCanonicalError> {
    encoder.count(rules.len())?;
    for rule in rules {
        encode_rule(encoder, rule)?;
    }
    Ok(())
}

fn encode_rule(
    encoder: &mut Encoder,
    rule: &FieldRuleDto,
) -> Result<(), SchemaMigrationCanonicalError> {
    let FieldRuleDto {
        path,
        r#type,
        constraints,
        keyset_safe: _,
    } = rule;
    strings(encoder, path)?;
    encoder.str(r#type)?;
    let crate::admin::types::schema_ops::ConstraintsDto {
        required,
        nullable,
        unsigned,
        min,
        max,
        len,
        max_len,
        min_len,
        one_of,
        default,
        array_of,
        scalar: _,
        format,
        compare: _,
        foreign_key,
        unique,
        auto_now,
        auto_now_add,
    } = constraints;
    encode_optional_bool(encoder, required.as_ref());
    encode_optional_bool(encoder, nullable.as_ref());
    encode_optional_bool(encoder, unsigned.as_ref());
    encode_num(encoder, min.as_ref())?;
    encode_num(encoder, max.as_ref())?;
    encode_optional_u64(encoder, len.as_ref());
    encode_optional_u64(encoder, max_len.as_ref());
    encode_optional_u64(encoder, min_len.as_ref());
    encoder.optional(one_of.as_ref(), |e, values| {
        e.count(values.len())?;
        for value in values {
            encode_value(e, value, 0)?;
        }
        Ok(())
    })?;
    encoder.optional(default.as_ref(), |e, value| {
        encode_filter_value(e, value, 0)
    })?;
    encode_optional_str(encoder, array_of.as_ref())?;
    if let Some(format) = format {
        encoder.optional(Some(format), |e, value| e.str(value))?;
    } else {
        encoder.optional(None::<&String>, |e, value| e.str(value))?;
    }
    encoder.optional(foreign_key.as_ref(), |e, key| {
        e.str(&key.ref_table)?;
        e.str(&key.ref_field)?;
        encode_fk_action(e, key.on_delete);
        encode_fk_action(e, key.on_update);
        Ok(())
    })?;
    encode_optional_bool(encoder, unique.as_ref());
    encoder.bool(*auto_now);
    encoder.bool(*auto_now_add);
    Ok(())
}

fn encode_optional_bool(encoder: &mut Encoder, value: Option<&bool>) {
    match value {
        None => encoder.u8(0),
        Some(value) => {
            encoder.u8(1);
            encoder.bool(*value);
        }
    }
}
fn encode_optional_u64(encoder: &mut Encoder, value: Option<&u64>) {
    match value {
        None => encoder.u8(0),
        Some(value) => {
            encoder.u8(1);
            encoder.u64(*value);
        }
    }
}
fn encode_fk_action(encoder: &mut Encoder, value: crate::admin::types::fk_action::FkAction) {
    encoder.u8(match value {
        crate::admin::types::fk_action::FkAction::NoAction => 0,
        crate::admin::types::fk_action::FkAction::Restrict => 1,
        crate::admin::types::fk_action::FkAction::Cascade => 2,
        crate::admin::types::fk_action::FkAction::SetNull => 3,
    });
}

fn encode_num(
    encoder: &mut Encoder,
    value: Option<&NumDto>,
) -> Result<(), SchemaMigrationCanonicalError> {
    encoder.optional(value, |e, value| match value {
        NumDto::Int(value) => {
            e.u8(0);
            e.i64(*value);
            Ok(())
        }
        NumDto::F64(number) => {
            if !number.is_finite() {
                return Err(SchemaMigrationCanonicalError::InvalidModel(
                    "non-finite number".to_owned(),
                ));
            }
            e.u8(1);
            e.bytes.extend_from_slice(&number.to_bits().to_be_bytes());
            Ok(())
        }
    })
}

fn encode_filter_value(
    encoder: &mut Encoder,
    value: &FilterValue,
    depth: usize,
) -> Result<(), SchemaMigrationCanonicalError> {
    if depth > 64 {
        return Err(SchemaMigrationCanonicalError::InvalidModel(
            "filter value nesting exceeds 64".to_owned(),
        ));
    }
    match value {
        FilterValue::Null => encoder.u8(0),
        FilterValue::Bool(value) => {
            encoder.u8(1);
            encoder.bool(*value);
        }
        FilterValue::Int(value) => {
            encoder.u8(2);
            encoder.i64(*value);
        }
        FilterValue::Float(value) => {
            if !value.is_finite() {
                return Err(SchemaMigrationCanonicalError::InvalidModel(
                    "non-finite filter float".to_owned(),
                ));
            }
            encoder.u8(3);
            encoder
                .bytes
                .extend_from_slice(&value.to_bits().to_be_bytes());
        }
        FilterValue::Binary(value) => {
            encoder.u8(4);
            encoder.bytes(value)?;
        }
        FilterValue::String(value) => {
            encoder.u8(5);
            encoder.str(value)?;
        }
        FilterValue::Array(values) => {
            encoder.u8(6);
            encoder.count(values.len())?;
            for value in values {
                encode_filter_value(encoder, value, depth + 1)?;
            }
        }
        FilterValue::FieldRef { .. }
        | FilterValue::QueryRef { .. }
        | FilterValue::FnCall { .. }
        | FilterValue::Expr { .. }
        | FilterValue::Cond { .. }
        | FilterValue::Param { .. } => {
            return Err(SchemaMigrationCanonicalError::InvalidModel(
                "dynamic filter value is excluded".to_owned(),
            ))
        }
    }
    Ok(())
}

pub(super) fn encode_value(
    encoder: &mut Encoder,
    value: &QueryValue,
    depth: usize,
) -> Result<(), SchemaMigrationCanonicalError> {
    if depth > 64 {
        return Err(SchemaMigrationCanonicalError::InvalidModel(
            "query value nesting exceeds 64".to_owned(),
        ));
    }
    match value {
        QueryValue::Null => encoder.u8(0),
        QueryValue::Bool(value) => {
            encoder.u8(1);
            encoder.bool(*value);
        }
        QueryValue::Int(value) => {
            encoder.u8(2);
            encoder.i64(*value);
        }
        QueryValue::F64(value) => {
            if !value.is_finite() {
                return Err(SchemaMigrationCanonicalError::InvalidModel(
                    "non-finite query float".to_owned(),
                ));
            }
            encoder.u8(3);
            encoder
                .bytes
                .extend_from_slice(&value.to_bits().to_be_bytes());
        }
        QueryValue::Big(value) => {
            encoder.u8(4);
            encoder.str(&value.to_string())?;
        }
        QueryValue::Dec(value) => {
            encoder.u8(5);
            encoder.str(&value.to_string())?;
        }
        QueryValue::Str(value) => {
            encoder.u8(6);
            encoder.str(value)?;
        }
        QueryValue::Bin(value) => {
            encoder.u8(7);
            encoder.bytes(value)?;
        }
        QueryValue::List(values) => {
            encoder.u8(8);
            encoder.count(values.len())?;
            for value in values {
                encode_value(encoder, value, depth + 1)?;
            }
        }
        QueryValue::Set(values) => {
            encoder.u8(9);
            let mut items = Vec::with_capacity(values.len());
            for value in values {
                let mut item = Encoder::new();
                encode_value(&mut item, value, depth + 1)?;
                items.push(item.bytes);
            }
            items.sort();
            if items.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(SchemaMigrationCanonicalError::InvalidModel(
                    "duplicate canonical set values".to_owned(),
                ));
            }
            encoder.count(items.len())?;
            for item in items {
                encoder.bytes(&item)?;
            }
        }
        QueryValue::Map(values) => {
            encoder.u8(10);
            let mut entries = Vec::with_capacity(values.len());
            for (key, value) in values {
                let mut encoded_value = Encoder::new();
                encode_value(&mut encoded_value, value, depth + 1)?;
                entries.push((key.as_bytes(), encoded_value.bytes));
            }
            entries.sort_by(|left, right| left.0.cmp(right.0));
            encoder.count(entries.len())?;
            for (key, value) in entries {
                let key = std::str::from_utf8(key).map_err(|_| {
                    SchemaMigrationCanonicalError::InvalidModel("map key is not UTF-8".to_owned())
                })?;
                encoder.str(key)?;
                encoder.bytes(&value)?;
            }
        }
    }
    Ok(())
}
