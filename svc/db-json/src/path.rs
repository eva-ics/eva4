use eva_common::{value::Value, EResult, Error};

/// Converts a simple JSONPath (e.g. `$.a[0].b`) into PostgreSQL jsonb syntax
/// (e.g. `data -> 'a' -> 0 -> 'b'`)
///
/// `column` is the jsonb column name (e.g. "data")
pub fn jsonpath_to_pg(column: &str, path: Option<Value>) -> EResult<String> {
    let Some(path) = path else {
        return Ok(column.to_owned());
    };
    let Value::String(path) = path else {
        return Err(Error::failed("Invalid JSONPath: must be a string"));
    };
    if !path.starts_with("$.") {
        return Err(Error::failed("Invalid JSONPath: must start with '$.'"));
    }

    let mut sql = String::from(column);
    let mut chars = path[2..].chars().peekable();
    let mut buf = String::new();

    while let Some(c) = chars.next() {
        match c {
            '.' => {
                flush_key(&mut sql, &mut buf)?;
            }
            '[' => {
                flush_key(&mut sql, &mut buf)?;
                let index = parse_index(&mut chars)?;
                sql.push_str(" -> ");
                sql.push_str(&index.to_string());
            }
            _ => buf.push(c),
        }
    }

    flush_key(&mut sql, &mut buf)?;
    Ok(sql)
}

fn flush_key(sql: &mut String, buf: &mut String) -> EResult<()> {
    if buf.is_empty() {
        return Ok(());
    }

    if !is_valid_identifier(buf) {
        return Err(Error::failed(format!("Invalid JSONPath key: '{}'", buf)));
    }

    sql.push_str(" -> '");
    sql.push_str(&buf.replace('\'', "''"));
    sql.push('\'');
    buf.clear();
    Ok(())
}

fn parse_index<I>(chars: &mut std::iter::Peekable<I>) -> EResult<usize>
where
    I: Iterator<Item = char>,
{
    let mut num = String::new();

    while let Some(&c) = chars.peek() {
        chars.next();
        if c == ']' {
            break;
        }
        if !c.is_ascii_digit() {
            return Err(Error::failed(
                "Invalid JSONPath: array index must be a number",
            ));
        }
        num.push(c);
    }

    num.parse()
        .map_err(|_| Error::failed("Invalid JSONPath: array index parse error"))
}

fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}
