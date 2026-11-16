use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::value::{QueryValue, Value};

use super::value::LogicalValue;

pub fn from_query_value(value: &QueryValue) -> Result<LogicalValue, String> {
    convert_value(value, 0)
}

pub fn into_query_value(value: LogicalValue) -> Result<QueryValue, String> {
    from_logical(value, 0)
}

fn convert_value(value: &QueryValue, depth: usize) -> Result<LogicalValue, String> {
    if depth >= 64 {
        return Err("logical value depth exceeds 64".into());
    }
    Ok(match value {
        Value::Null => LogicalValue::Null,
        Value::Bool(value) => LogicalValue::Bool(*value),
        Value::Int(value) => LogicalValue::Int(*value),
        Value::F64(value) => LogicalValue::F64(value.to_bits()),
        Value::Dec(value) => {
            let mantissa = value.mantissa();
            let coefficient = mantissa.unsigned_abs();
            if coefficient >= (1_u128 << 96) {
                return Err("decimal coefficient outside supported range".into());
            }
            if coefficient == 0 && mantissa < 0 {
                return Err("negative zero decimal is not canonical".into());
            }
            LogicalValue::Decimal {
                coefficient,
                scale: value.scale(),
                negative: value.is_sign_negative(),
            }
        }
        Value::Big(value) => LogicalValue::Big(value.to_string()),
        Value::Str(value) => LogicalValue::Str(value.clone()),
        Value::Bin(value) => LogicalValue::Bin(value.clone()),
        Value::List(values) => LogicalValue::List(
            values
                .iter()
                .map(|value| convert_value(value, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Value::Set(values) => LogicalValue::Set(
            values
                .iter()
                .map(|value| convert_value(value, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Value::Map(values) => LogicalValue::Map(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), convert_value(value, depth + 1)?)))
                .collect::<Result<_, String>>()?,
        ),
    })
}

fn from_logical(value: LogicalValue, depth: usize) -> Result<QueryValue, String> {
    if depth >= 64 {
        return Err("logical value depth exceeds 64".into());
    }
    Ok(match value {
        LogicalValue::Null => Value::Null,
        LogicalValue::Bool(value) => Value::Bool(value),
        LogicalValue::Int(value) => Value::Int(value),
        LogicalValue::F64(bits) => Value::F64(f64::from_bits(bits)),
        LogicalValue::Decimal {
            coefficient,
            scale,
            negative,
        } => {
            if coefficient >= (1_u128 << 96) || scale > 28 {
                return Err("decimal payload outside supported range".into());
            }
            if coefficient == 0 && negative {
                return Err("negative zero decimal is not canonical".into());
            }
            let magnitude = coefficient.to_string();
            let mut text = if scale == 0 {
                magnitude
            } else {
                let scale = usize::try_from(scale).map_err(|error| error.to_string())?;
                let padded = format!("{:0>width$}", magnitude, width = scale + 1);
                let split = padded.len() - scale;
                format!("{}.{}", &padded[..split], &padded[split..])
            };
            if negative {
                text.insert(0, '-');
            }
            Value::Dec(text.parse().map_err(|_| "invalid decimal".to_owned())?)
        }
        LogicalValue::Big(value) => Value::Big(value.parse().map_err(|error| format!("{error}"))?),
        LogicalValue::Str(value) => Value::Str(value),
        LogicalValue::Bin(value) => Value::Bin(value),
        LogicalValue::List(values) => Value::List(
            values
                .into_iter()
                .map(|value| from_logical(value, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        LogicalValue::Set(values) => {
            let mut set = new_set();
            for value in values {
                if !set.insert(from_logical(value, depth + 1)?) {
                    return Err("duplicate logical Set value".into());
                }
            }
            Value::Set(set)
        }
        LogicalValue::Map(values) => {
            let mut map = new_map();
            for (key, value) in values {
                if map.insert(key, from_logical(value, depth + 1)?).is_some() {
                    return Err("duplicate logical Map key".into());
                }
            }
            Value::Map(map)
        }
    })
}
