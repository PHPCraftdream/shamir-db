//! Helpers shared by native stored-call database-route tests.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use shamir_db::{DbError, ShamirDb};
use shamir_engine::function::{
    FnBatch, FnCtx, FunctionError, FunctionMeta, HttpRequest, NetGatewayError, Params, Security,
    ShamirFunction, Visibility,
};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::write::{doc, insert};
use shamir_types::access::{Actor, ResourceMeta};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

pub(crate) const DB_NAME: &str = "testdb";
pub(crate) const TABLE: &str = "secrets";
pub(crate) const OWNER: u64 = 1001;
pub(crate) const CALLER: u64 = 2002;

pub(crate) fn user(id: u64) -> Actor {
    Actor::User(id)
}

pub(crate) async fn setup() -> ShamirDb {
    let db = ShamirDb::init_memory().await.expect("in-memory database");
    db.create_db(DB_NAME).await.expect("create database");
    db
}

pub(crate) async fn register_native(
    db: &ShamirDb,
    name: &str,
    body: Arc<dyn ShamirFunction>,
    owner: Actor,
    mode: u16,
    security: Security,
    grants: (Vec<String>, Vec<String>),
) {
    let (secret_grants, net_grants) = grants;
    db.functions()
        .register(name, body)
        .unwrap_or_else(|error| panic!("register {name}: {error}"));

    let mut fields = new_map();
    fields.insert("name".into(), QueryValue::Str(name.into()));
    fields.insert("kind".into(), QueryValue::Str("native".into()));
    let mut record = QueryValue::Map(fields);
    FunctionMeta::new(Visibility::Public, security, secret_grants, net_grants)
        .inject_into(&mut record);
    db.system_store()
        .save_function(
            name,
            &record,
            &ResourceMeta {
                owner,
                group: None,
                mode,
            },
        )
        .await
        .unwrap_or_else(|error| panic!("catalogue {name}: {error}"));
}

pub(crate) async fn invoke_as(
    db: &ShamirDb,
    name: &str,
    actor: Actor,
) -> Result<QueryValue, DbError> {
    db.invoke_function_as(name, Params::new(), actor).await
}

pub(crate) async fn invoke_in_db_as(
    db: &ShamirDb,
    name: &str,
    actor: Actor,
) -> Result<QueryValue, DbError> {
    db.invoke_function_in_db_as(DB_NAME, "main", name, Params::new(), actor)
        .await
}

pub(crate) async fn setup_secrets_table(db: &ShamirDb, table_owner: Actor) {
    let mut create = Batch::new();
    create.id("create-secrets");
    create.create_repo(
        "repo",
        ddl::create_repo("main").engine("in_memory").tables([TABLE]),
    );
    db.execute(
        DB_NAME,
        &create.to_request_via_msgpack().expect("create request"),
    )
    .await
    .expect("create repository and table");

    let open = ResourceMeta::open();
    db.set_resource_meta(
        &shamir_types::access::ResourcePath::database(DB_NAME),
        &open,
    )
    .await
    .expect("open database ancestor");
    db.set_resource_meta(
        &shamir_types::access::ResourcePath::store(DB_NAME, "main"),
        &open,
    )
    .await
    .expect("open store ancestor");

    let mut seed = Batch::new();
    seed.id("seed-secret");
    let secret_row = doc()
        .set("id", 1_i64)
        .expect("secret row id")
        .set("value", "secret")
        .expect("secret row value");
    seed.insert("insert", insert(TABLE).row(secret_row));
    db.execute(
        DB_NAME,
        &seed.to_request_via_msgpack().expect("seed request"),
    )
    .await
    .expect("seed secret row");

    db.set_resource_meta(
        &shamir_types::access::ResourcePath::table(DB_NAME, "main", TABLE),
        &ResourceMeta {
            owner: table_owner,
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("restrict secret table to owner");
}

pub(crate) struct Forward(pub(crate) &'static str);

pub(crate) struct NativeRootCall {
    pub(crate) calls: Arc<AtomicUsize>,
    pub(crate) recurse: bool,
}

#[async_trait]
impl ShamirFunction for NativeRootCall {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.recurse {
            ctx.call_stored("self_cycle", batch, params).await
        } else {
            Ok(QueryValue::Int(1))
        }
    }
}

#[async_trait]
impl ShamirFunction for Forward {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        ctx.call_stored(self.0, batch, params).await
    }
}

pub(crate) struct ActorProbe;

#[async_trait]
impl ShamirFunction for ActorProbe {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        Ok(QueryValue::Int(ctx.actor().to_owner_id() as i64))
    }
}

pub(crate) struct DefinerTableReader;

#[async_trait]
impl ShamirFunction for DefinerTableReader {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let gateway = ctx
            .db_gateway()
            .ok_or_else(|| FunctionError::Compute("target DB gateway missing".into()))?;
        let rows = gateway
            .query("main", TABLE, None)
            .await
            .map_err(|error| FunctionError::Compute(error.to_string()))?;
        Ok(QueryValue::Int(rows.len() as i64))
    }
}

/// Whether `curl` (used by the production net gateway) is on PATH.
pub(crate) fn curl_available() -> bool {
    std::process::Command::new("curl")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub(crate) struct LoopbackHttpFixture {
    pub(crate) parent_only_url: String,
    pub(crate) target_only_url: String,
    parent_only: TcpListener,
    target_only: TcpListener,
}

impl LoopbackHttpFixture {
    pub(crate) async fn new() -> io::Result<Self> {
        let parent_only = TcpListener::bind("127.0.0.1:0").await?;
        let target_only = TcpListener::bind("127.0.0.2:0").await?;
        let parent_address = parent_only.local_addr()?;
        let target_address = target_only.local_addr()?;
        Ok(Self {
            parent_only_url: format!("http://{parent_address}/policy"),
            target_only_url: format!("http://{target_address}/policy"),
            parent_only,
            target_only,
        })
    }

    pub(crate) async fn serve(&self) -> io::Result<()> {
        tokio::try_join!(
            respond_once(&self.parent_only, "parent-only"),
            respond_once(&self.target_only, "target-only"),
        )?;
        Ok(())
    }
}

async fn respond_once(listener: &TcpListener, body: &'static str) -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let (mut stream, _) = listener.accept().await?;
        respond(&mut stream, body).await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "loopback HTTP request timed out"))?
}

async fn respond(stream: &mut TcpStream, body: &'static str) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let stream = reader.get_mut();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

pub(crate) struct SeedParentNetGateway {
    pub(crate) target: &'static str,
    pub(crate) parent_only_url: String,
    pub(crate) target_only_url: String,
}

#[async_trait]
impl ShamirFunction for SeedParentNetGateway {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let parent_allows_parent_url =
            fetch_policy(ctx, &self.parent_only_url, "parent-only").await?;
        let parent_allows_target_url =
            fetch_policy(ctx, &self.target_only_url, "target-only").await?;
        batch.put(
            "parent_allows_parent_url",
            QueryValue::Bool(parent_allows_parent_url),
        );
        batch.put(
            "parent_allows_target_url",
            QueryValue::Bool(parent_allows_target_url),
        );
        batch.put(
            "parent_only_url",
            QueryValue::Str(self.parent_only_url.clone()),
        );
        batch.put(
            "target_only_url",
            QueryValue::Str(self.target_only_url.clone()),
        );
        ctx.call_stored(self.target, batch, params).await
    }
}

async fn fetch_policy(ctx: &FnCtx, url: &str, expected_body: &str) -> Result<bool, FunctionError> {
    let gateway = ctx
        .net_gateway()
        .ok_or_else(|| FunctionError::Compute("network gateway missing".into()))?;
    match gateway
        .fetch(HttpRequest {
            method: "GET".into(),
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        })
        .await
    {
        Ok(response)
            if response.status == 200 && response.body.as_slice() == expected_body.as_bytes() =>
        {
            Ok(true)
        }
        Err(NetGatewayError::NotAllowed(_)) => Ok(false),
        Ok(response) => Err(FunctionError::Compute(format!(
            "unexpected network policy response: status {} body {:?}",
            response.status, response.body
        ))),
        Err(error) => Err(FunctionError::Compute(format!(
            "unexpected network policy fetch error: {error}"
        ))),
    }
}

fn batch_string(batch: &FnBatch, key: &str) -> Result<String, FunctionError> {
    match batch.get(key) {
        Some(QueryValue::Str(value)) => Ok(value),
        _ => Err(FunctionError::Compute(format!("missing {key} in batch"))),
    }
}

fn batch_bool(batch: &FnBatch, key: &str) -> Result<bool, FunctionError> {
    match batch.get(key) {
        Some(QueryValue::Bool(value)) => Ok(value),
        _ => Err(FunctionError::Compute(format!(
            "missing or invalid {key} in batch"
        ))),
    }
}

pub(crate) struct CapabilityProbe;

#[async_trait]
impl ShamirFunction for CapabilityProbe {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let secret_grant_present = ctx
            .secret_grants()
            .contains(&"SHAMIR_TEST_SECRET".to_string());
        let mut fields = new_map();
        fields.insert(
            "actor".into(),
            QueryValue::Int(ctx.actor().to_owner_id() as i64),
        );
        fields.insert("repo".into(), QueryValue::Str(ctx.repo().to_string()));
        fields.insert(
            "db_gateway_present".into(),
            QueryValue::Bool(ctx.db_gateway().is_some()),
        );
        fields.insert(
            "secret_grant".into(),
            QueryValue::Bool(secret_grant_present),
        );
        Ok(QueryValue::Map(fields))
    }
}

pub(crate) struct NetPolicyProbe;

#[async_trait]
impl ShamirFunction for NetPolicyProbe {
    async fn call(
        &self,
        ctx: &FnCtx,
        batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let secret_grant_present = ctx
            .secret_grants()
            .contains(&"SHAMIR_TEST_SECRET".to_string());
        let mut fields = new_map();
        fields.insert(
            "secret_grant".into(),
            QueryValue::Bool(secret_grant_present),
        );
        fields.insert(
            "parent_allows_parent_url".into(),
            QueryValue::Bool(batch_bool(batch, "parent_allows_parent_url")?),
        );
        fields.insert(
            "parent_allows_target_url".into(),
            QueryValue::Bool(batch_bool(batch, "parent_allows_target_url")?),
        );
        let parent_url = batch_string(batch, "parent_only_url")?;
        let target_url = batch_string(batch, "target_only_url")?;
        fields.insert(
            "target_allows_parent_url".into(),
            QueryValue::Bool(fetch_policy(ctx, &parent_url, "parent-only").await?),
        );
        fields.insert(
            "target_allows_target_url".into(),
            QueryValue::Bool(fetch_policy(ctx, &target_url, "target-only").await?),
        );
        Ok(QueryValue::Map(fields))
    }
}
