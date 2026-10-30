//! Consumer signatures for all four attribute macros using an explicit SDK path.
extern crate shamir_sdk as renamed;

mod scalar {
    use super::renamed;
    type Alias = renamed::Params;
    #[renamed::scalar(crate = renamed)]
    async fn scalar(params: Alias) -> renamed::Result<renamed::Value> {
        let _ctx = renamed::Ctx::new();
        let _ = params;
        Ok(renamed::Value::Null)
    }
}

mod function {
    use super::renamed;
    #[renamed::function(crate = renamed)]
    async fn function(
        _ctx: renamed::Ctx,
        _batch: renamed::Batch,
        _params: renamed::Params,
    ) -> renamed::Result<renamed::Value> {
        Ok(renamed::Value::Null)
    }
}

mod procedure {
    use super::renamed;
    #[renamed::procedure(crate = renamed)]
    async fn procedure(
        ctx: renamed::Ctx,
        _params: renamed::Params,
    ) -> renamed::Result<renamed::Value> {
        let rows = ctx.db().table("users").query(None)?;
        Ok(renamed::Value::List(rows))
    }
}

mod validator {
    use super::renamed;
    #[renamed::validator(crate = renamed)]
    async fn validator(
        _record: renamed::Value,
        _old: Option<renamed::Value>,
        _ctx: renamed::Ctx,
    ) -> renamed::Validation {
        renamed::Validation::accept()
    }
}

#[test]
fn renamed_sdk_attribute_consumers_compile() {}
