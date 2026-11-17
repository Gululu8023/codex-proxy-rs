//! 同进程 veil 边界：身份与生命周期由 CPRS 提供，脱敏、映射和恢复复用上游库。

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, mpsc, oneshot};
use veil_engine::{creator::Creator, rules::RuleSet, walk::WalkError};
use veil_store::SqliteStore;
use zeroize::Zeroizing;

pub const REVISION: &str = "82f9290022144c82609aaad149d81693c4a563ec+cprs-library-3";

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub data_directory: PathBuf,
    pub rules_file: Option<PathBuf>,
    pub max_mappings: u64,
    pub max_parallel_operations: usize,
    /// 自动准备全新映射卷；已有卷缺少密钥、映射库或清单时仍须恢复备份。
    pub initialize_store: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_directory: PathBuf::from("veil"),
            rules_file: None,
            max_mappings: 1_000_000,
            max_parallel_operations: 32,
            initialize_store: true,
        }
    }
}

impl Config {
    pub fn resolve_paths(&mut self, runtime_directory: &Path, source_directory: &Path) {
        if self.data_directory.is_relative() {
            self.data_directory = runtime_directory.join(&self.data_directory);
        }
        if let Some(path) = &mut self.rules_file
            && path.is_relative()
        {
            *path = source_directory.join(&*path);
        }
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    #[error("veil_not_configured")]
    NotConfigured,
    #[error("veil_invalid_rules")]
    InvalidRules,
    #[error("veil_rule_evaluation_failed")]
    RuleEvaluation,
    #[error("veil_store_unavailable")]
    StoreUnavailable,
    #[error("veil_mapping_capacity")]
    MappingCapacity,
    #[error("veil_mapping_missing")]
    MappingMissing,
    #[error("veil_invalid_stream")]
    InvalidStream,
    #[error("veil_buffer_limit")]
    BufferLimit,
    #[error("veil_busy")]
    Busy,
    #[error("veil_worker_unavailable")]
    WorkerUnavailable,
}

impl From<WalkError> for Error {
    fn from(error: WalkError) -> Self {
        match error {
            WalkError::RuleEvaluation => Self::RuleEvaluation,
            WalkError::MappingWrite | WalkError::StoreUnavailable => Self::StoreUnavailable,
            WalkError::MappingCapacity => Self::MappingCapacity,
            WalkError::MissingMapping => Self::MappingMissing,
            WalkError::InvalidStream => Self::InvalidStream,
            WalkError::BufferLimit => Self::BufferLimit,
        }
    }
}

/// 只接受认证后内部 ID；上游账号轮换和 Client Key 密钥轮换不改变用户映射空间。
pub enum Scope<'a> {
    ClientKey(&'a str),
    Diagnostic(&'a str),
    Review(&'a str),
}

impl Scope<'_> {
    fn creator(self) -> Creator {
        let (domain, identity) = match self {
            Self::ClientKey(id) => ("client-key", id),
            Self::Diagnostic(id) => ("diagnostic", id),
            Self::Review(id) => ("review", id),
        };
        Creator::from_secret(&format!("cprs:veil:v1:{domain}\0{identity}"))
    }
}

/// 控制请求同样经过就绪边界，认证字段和无正文请求不做通用字符串替换。
#[derive(Clone, Copy)]
pub enum ControlPurpose {
    OAuth,
    Profile,
    Quota,
    Models,
}

struct State {
    store: SqliteStore,
    checkpoint_cipher: veil_store::CheckpointCipher,
    rules: RuleSet,
    rules_revision: String,
    state_fingerprint: String,
    store_generation: String,
    key_fingerprint: String,
    max_mappings: u64,
    packs: veil::config::PacksConfig,
}

pub struct Runtime {
    state: Result<Arc<State>, Error>,
    data_directory: PathBuf,
    rules_file: Option<PathBuf>,
    slots: Arc<Semaphore>,
    last_error: Arc<Mutex<Option<Failure>>>,
    metrics: Arc<Metrics>,
}

#[derive(Default)]
struct Metrics {
    redactions: AtomicU64,
    restorations: AtomicU64,
    errors: AtomicU64,
    processing_ms: AtomicU64,
}
#[derive(Clone, Copy)]
struct Failure {
    error: Error,
    observed_at: SystemTime,
}
impl Metrics {
    fn observe(&self, restore: bool, failed: bool, started: Instant) {
        let count = if restore {
            &self.restorations
        } else {
            &self.redactions
        };
        count.fetch_add(1, Ordering::Relaxed);
        if failed {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        self.processing_ms.fetch_add(
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub ready: bool,
    pub revision: &'static str,
    pub rules_revision: Option<String>,
    pub rule_count: usize,
    pub max_mappings: u64,
    /// 主密钥的域分离指纹，供恢复时与备份清单核对；不能反推密钥。
    pub key_fingerprint: Option<String>,
    pub available_slots: usize,
    pub mapping_count: Option<u64>,
    pub last_error: Option<Error>,
    #[serde(skip)]
    pub last_error_at: Option<SystemTime>,
    pub automatic_eviction: bool,
    pub redaction_operations: u64,
    pub restoration_operations: u64,
    pub failed_operations: u64,
    pub processing_ms: u64,
    pub packs: Option<veil::config::PacksConfig>,
    pub categories: std::collections::BTreeMap<String, usize>,
}

impl Runtime {
    /// 初始化失败仅关闭 Claude 数据面，保留管理端和其他 Provider。
    pub async fn initialize(config: Config) -> Arc<Self> {
        let parallelism = config.max_parallel_operations;
        let (data_directory, rules_file) =
            (config.data_directory.clone(), config.rules_file.clone());
        let state = tokio::task::spawn_blocking(move || initialize(config))
            .await
            .unwrap_or(Err(Error::WorkerUnavailable))
            .map(Arc::new);
        let error = state.as_ref().err().map(|error| Failure {
            error: *error,
            observed_at: SystemTime::now(),
        });
        Arc::new(Self {
            state,
            data_directory,
            rules_file,
            slots: Arc::new(Semaphore::new(parallelism.min(256))),
            last_error: Arc::new(Mutex::new(error)),
            metrics: Arc::default(),
        })
    }

    pub fn ensure_ready(&self) -> Result<(), Error> {
        self.state.as_ref().map_err(|error| *error)?;
        if matches!(
            *self.last_error.lock().expect("veil error state"),
            Some(Failure {
                error: Error::StoreUnavailable,
                ..
            })
        ) {
            return Err(Error::StoreUnavailable);
        }
        Ok(())
    }

    pub fn control(&self, _purpose: ControlPurpose) -> Result<(), Error> {
        self.ensure_ready()
    }
    /// 复用持久主密钥的独立加密域；恢复材料不进入脱敏映射表或诊断日志。
    pub fn seal_checkpoint(&self, binding: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, Error> {
        self.ensure_ready()?;
        self.state
            .as_ref()
            .map_err(|error| *error)?
            .checkpoint_cipher
            .seal(binding, plaintext)
            .map_err(|_| Error::StoreUnavailable)
    }
    pub fn open_checkpoint(&self, binding: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, Error> {
        self.ensure_ready()?;
        self.state
            .as_ref()
            .map_err(|error| *error)?
            .checkpoint_cipher
            .open(binding, ciphertext)
            .map_err(|_| Error::StoreUnavailable)
    }
    pub fn rules_revision(&self) -> Option<&str> {
        self.state
            .as_ref()
            .ok()
            .map(|state| state.rules_revision.as_str())
    }
    /// 不泄露主密钥；规则、密钥或独立映射卷变化都会使 Thread 的恢复边界失效。
    pub fn state_fingerprint(&self) -> Option<&str> {
        self.state
            .as_ref()
            .ok()
            .map(|state| state.state_fingerprint.as_str())
    }

    pub fn session(&self, scope: Scope<'_>) -> Result<Session, Error> {
        self.ensure_ready()?;
        Ok(Session {
            state: self.state.as_ref().map_err(|error| *error)?.clone(),
            slots: self.slots.clone(),
            last_error: self.last_error.clone(),
            metrics: self.metrics.clone(),
            creator: scope.creator(),
        })
    }

    pub async fn health(&self) -> Health {
        let mut mapping_count = None;
        if let Ok(state) = &self.state
            && let Ok(slot) = self.slots.clone().try_acquire_owned()
        {
            let state = state.clone();
            let result = tokio::task::spawn_blocking(move || {
                let _slot = slot;
                state
                    .store
                    .mapping_count()
                    .map_err(|_| Error::StoreUnavailable)
            })
            .await
            .unwrap_or(Err(Error::WorkerUnavailable));
            mapping_count = remember(result, &self.last_error).ok();
        }
        let state = self.state.as_ref().ok();
        let failure = *self.last_error.lock().expect("veil error state");
        Health {
            ready: self.ensure_ready().is_ok(),
            revision: REVISION,
            rules_revision: state.map(|state| state.rules_revision.clone()),
            rule_count: state.map_or(0, |state| {
                state
                    .rules
                    .rules()
                    .iter()
                    .filter(|rule| rule.enabled)
                    .count()
            }),
            max_mappings: state.map_or(0, |state| state.max_mappings),
            key_fingerprint: state.map(|state| state.key_fingerprint.clone()),
            available_slots: self.slots.available_permits(),
            mapping_count,
            last_error: failure.map(|failure| failure.error),
            last_error_at: failure.map(|failure| failure.observed_at),
            automatic_eviction: false,
            redaction_operations: self.metrics.redactions.load(Ordering::Relaxed),
            restoration_operations: self.metrics.restorations.load(Ordering::Relaxed),
            failed_operations: self.metrics.errors.load(Ordering::Relaxed),
            processing_ms: self.metrics.processing_ms.load(Ordering::Relaxed),
            packs: state.map(|state| state.packs.clone()),
            categories: state.map_or_else(Default::default, |state| {
                let mut categories = std::collections::BTreeMap::new();
                for rule in state.rules.rules().iter().filter(|rule| rule.enabled) {
                    *categories.entry(rule.type_prefix.clone()).or_insert(0) += 1;
                }
                categories
            }),
        }
    }
}

fn initialize(config: Config) -> Result<State, Error> {
    if config.max_mappings == 0 || !(1..=256).contains(&config.max_parallel_operations) {
        return Err(Error::NotConfigured);
    }
    let rules_config = if let Some(path) = config.rules_file {
        let text = std::fs::read_to_string(path).map_err(|_| Error::InvalidRules)?;
        toml::from_str::<veil::Config>(&text).map_err(|_| Error::InvalidRules)?
    } else {
        veil::Config::default()
    };
    for rule in &rules_config.rules {
        if rule.id.is_empty()
            || rule.type_prefix.is_empty()
            || !rule
                .type_prefix
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(Error::InvalidRules);
        }
        if let Some(pattern) = &rule.pattern {
            fancy_regex::Regex::new(pattern).map_err(|_| Error::InvalidRules)?;
        }
        if rule.pattern.is_none()
            && rule.words.as_ref().is_none_or(Vec::is_empty)
            && rule.id != "ip"
            && rule.type_prefix != "IP"
        {
            return Err(Error::InvalidRules);
        }
    }
    let rules = veil::ruleset::ruleset_from_config(&rules_config);
    if !rules.rules().iter().any(|rule| rule.enabled) {
        return Err(Error::InvalidRules);
    }
    let rules_revision = hex::encode(Sha256::digest(
        serde_json::to_vec(&serde_json::json!({
            "revision": REVISION, "packs": rules_config.packs,
            "rules": rules_config.rules, "allowlist": rules_config.allowlist,
        }))
        .expect("serializable veil rules"),
    ));
    let mut directory = std::fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory
        .create(&config.data_directory)
        .map_err(unavailable)?;
    let database = config.data_directory.join("mappings.db");
    let initialized = config.data_directory.join("initialized");
    let revision_file = config.data_directory.join("rules-revision");
    let database_exists = database.try_exists().map_err(unavailable)?;
    let initialized_exists = initialized.try_exists().map_err(unavailable)?;
    let revision_exists = revision_file.try_exists().map_err(unavailable)?;
    let new_store = !database_exists && !initialized_exists && !revision_exists;
    // 先检查持久化痕迹，再允许生成密钥；部分丢失的旧卷不能被重新初始化为空卷。
    if new_store {
        if !config.initialize_store {
            return Err(Error::StoreUnavailable);
        }
    } else if !(database_exists && initialized_exists && revision_exists) {
        return Err(Error::StoreUnavailable);
    }
    // 只在全新卷使用库的排他创建逻辑；已有卷始终要求原密钥，不自动补建或覆盖。
    let mode = if new_store {
        veil::Mode::Desktop
    } else {
        veil::Mode::Server
    };
    let key = Zeroizing::new(
        veil::load_or_create(&config.data_directory, mode).map_err(|_| Error::NotConfigured)?,
    );
    let store = SqliteStore::open(database, &key)
        .map_err(|_| Error::StoreUnavailable)?
        .with_max_rows(config.max_mappings);
    if new_store && store.mapping_count().map_err(|_| Error::StoreUnavailable)? != 0 {
        return Err(Error::StoreUnavailable);
    }
    if revision_file.exists() {
        let persisted =
            std::fs::read_to_string(&revision_file).map_err(|_| Error::StoreUnavailable)?;
        // 规则变更会改变已签名历史的再次脱敏结果，禁止在同一映射卷上静默启用新规则。
        if persisted != rules_revision {
            return Err(Error::InvalidRules);
        }
    } else {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(revision_file)
            .and_then(|mut file| file.write_all(rules_revision.as_bytes()))
            .map_err(|_| Error::StoreUnavailable)?;
    }
    let generation = store_generation(&initialized)?;
    let key_fingerprint = hex::encode(Sha256::digest(key.as_ref()));
    let state_fingerprint = hex::encode(Sha256::digest(format!(
        "cprs:veil-state:v1\0{generation}\0{key_fingerprint}\0{rules_revision}"
    )));
    let mut digest = Sha256::new();
    digest.update(b"cprs:veil-key-fingerprint:v1\0");
    digest.update(key.as_ref());
    let key_fingerprint = hex::encode(digest.finalize());
    // 不启动 purge：自动 TTL/LRU 淘汰会使仍有效的工具调用和会话历史无法恢复。
    Ok(State {
        store,
        checkpoint_cipher: veil_store::CheckpointCipher::new(&key),
        rules,
        rules_revision,
        state_fingerprint,
        store_generation: generation,
        key_fingerprint,
        max_mappings: config.max_mappings,
        packs: rules_config.packs,
    })
}

/// 备份快照的格式标识；恢复时据此识别归档布局。
pub const SNAPSHOT_FORMAT: &str = "cprs-veil-snapshot/1";
/// 快照目录内的清单文件名。
pub const SNAPSHOT_MANIFEST: &str = "manifest.json";
const SNAPSHOT_RULES_FILE: &str = "rules.toml";

/// 一份映射卷快照的配套事实；主密钥本身不进入快照。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotManifest {
    pub format: &'static str,
    pub revision: &'static str,
    pub rules_revision: String,
    pub store_generation: String,
    pub key_fingerprint: String,
    pub mapping_count: u64,
    /// 快照内的规则文件名；使用内置规则时为空。
    pub rules_file: Option<&'static str>,
    pub created_at_unix: u64,
}

impl Runtime {
    /// 把映射库、`initialized`、`rules-revision`、自定义规则和清单写入一个新建目录。
    ///
    /// 映射卷从未初始化时返回 `Ok(None)`；卷已存在但运行时不可用时返回错误，
    /// 调用方不能据此生成缺少映射的备份。映射只增不删，因此在数据库快照之后执行即可覆盖其引用的全部映射。
    /// 快照失败不写入运行时错误状态，不影响 Claude 数据面。
    pub async fn snapshot(&self, destination: PathBuf) -> Result<Option<SnapshotManifest>, Error> {
        let initialized = self.data_directory.join("initialized");
        let database = self.data_directory.join("mappings.db");
        let exists = |path: &Path| path.try_exists().map_err(|_| Error::StoreUnavailable);
        if !exists(&initialized)? && !exists(&database)? {
            return Ok(None);
        }
        let state = self.state.as_ref().map_err(|error| *error)?.clone();
        let slot = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::WorkerUnavailable)?;
        let rules_file = self.rules_file.clone();
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            write_snapshot(
                &state,
                &database,
                &initialized,
                rules_file.as_deref(),
                &destination,
            )
        })
        .await
        .unwrap_or(Err(Error::WorkerUnavailable))
        .map(Some)
    }
}

fn write_snapshot(
    state: &State,
    database: &Path,
    initialized: &Path,
    rules_file: Option<&Path>,
    destination: &Path,
) -> Result<SnapshotManifest, Error> {
    use rusqlite::{Connection, OpenFlags};
    create_private_directory(destination)?;
    // 卷内清单必须与运行时一致，避免把被替换过的文件当作当前映射空间备份。
    let generation = std::fs::read_to_string(initialized).map_err(unavailable)?;
    let revision =
        std::fs::read_to_string(database.with_file_name("rules-revision")).map_err(unavailable)?;
    if generation != state.store_generation || revision != state.rules_revision {
        return Err(Error::StoreUnavailable);
    }
    let target = destination.join("mappings.db");
    let target_text = target.to_str().ok_or(Error::StoreUnavailable)?;
    // 独立连接在 WAL 读事务内复制一致快照，不持有运行时连接锁，也不阻塞并发写入。
    let source = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(unavailable)?;
    source
        .busy_timeout(std::time::Duration::from_secs(30))
        .map_err(unavailable)?;
    source
        .execute("VACUUM INTO ?1", [target_text])
        .map_err(unavailable)?;
    drop(source);
    let copy = Connection::open_with_flags(
        &target,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(unavailable)?;
    // 归档只保留单文件；恢复后由映射库重新启用 WAL。
    let _: String = copy
        .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
        .map_err(unavailable)?;
    let mapping_count = copy
        .query_row("SELECT COUNT(*) FROM mappings", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(unavailable)?;
    drop(copy);
    restrict(&target)?;
    write_private(&destination.join("initialized"), generation.as_bytes())?;
    write_private(&destination.join("rules-revision"), revision.as_bytes())?;
    let rules_file = match rules_file {
        Some(path) => {
            write_private(
                &destination.join(SNAPSHOT_RULES_FILE),
                &std::fs::read(path).map_err(unavailable)?,
            )?;
            Some(SNAPSHOT_RULES_FILE)
        }
        None => None,
    };
    let manifest = SnapshotManifest {
        format: SNAPSHOT_FORMAT,
        revision: REVISION,
        rules_revision: revision,
        store_generation: generation,
        key_fingerprint: state.key_fingerprint.clone(),
        mapping_count: u64::try_from(mapping_count).map_err(|_| Error::StoreUnavailable)?,
        rules_file,
        created_at_unix: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
    };
    let text = serde_json::to_vec_pretty(&manifest).map_err(|_| Error::StoreUnavailable)?;
    write_private(&destination.join(SNAPSHOT_MANIFEST), &text)?;
    Ok(manifest)
}

fn unavailable<E>(_: E) -> Error {
    Error::StoreUnavailable
}

fn create_private_directory(path: &Path) -> Result<(), Error> {
    let builder = &mut std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| Error::StoreUnavailable)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| Error::StoreUnavailable)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| Error::StoreUnavailable)
}

fn restrict(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| Error::StoreUnavailable)?;
    }
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| Error::StoreUnavailable)
}

fn store_generation(path: &Path) -> Result<String, Error> {
    if !path.exists() {
        use std::io::Write;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| Error::StoreUnavailable)?;
        let generation = hex::encode(random);
        let temporary = path.with_file_name(format!("initializing-{generation}"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| Error::StoreUnavailable)?;
        file.write_all(generation.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|_| Error::StoreUnavailable)?;
        // hard_link publishes a complete manifest without replacing another initializer's generation.
        let result = std::fs::hard_link(&temporary, path);
        std::fs::remove_file(&temporary).map_err(|_| Error::StoreUnavailable)?;
        if result
            .as_ref()
            .is_err_and(|error| error.kind() != std::io::ErrorKind::AlreadyExists)
        {
            return Err(Error::StoreUnavailable);
        }
    }
    let generation = std::fs::read_to_string(path).map_err(|_| Error::StoreUnavailable)?;
    if generation.len() != 64 || !generation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::StoreUnavailable);
    }
    Ok(generation)
}

#[derive(Clone)]
pub struct Session {
    state: Arc<State>,
    slots: Arc<Semaphore>,
    last_error: Arc<Mutex<Option<Failure>>>,
    creator: Creator,
    metrics: Arc<Metrics>,
}

impl Session {
    pub async fn redact(&self, value: Value) -> Result<Value, Error> {
        self.transform(value, false).await
    }
    pub async fn restore(&self, value: Value) -> Result<Value, Error> {
        self.transform(value, true).await
    }

    async fn transform(&self, value: Value, restore: bool) -> Result<Value, Error> {
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let state = self.state.clone();
        let creator = self.creator;
        let errors = self.last_error.clone();
        let metrics = self.metrics.clone();
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            let started = Instant::now();
            let result = if restore {
                veil_engine::anthropic::restore_json(&value, &state.store, &creator)
            } else {
                veil_engine::anthropic::desensitize_json(
                    &value,
                    &state.rules,
                    &state.store,
                    &creator,
                )
            };
            metrics.observe(restore, result.is_err(), started);
            remember(result.map_err(Error::from), &errors)
        })
        .await
        .map_err(|_| Error::WorkerUnavailable)?
    }

    pub fn stream(&self) -> Result<Stream, Error> {
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let (sender, mut receiver) = mpsc::channel::<StreamJob>(1);
        let state = self.state.clone();
        let creator = self.creator;
        let errors = self.last_error.clone();
        let metrics = self.metrics.clone();
        // 每条流只有一个恢复器；接收端丢弃后阻塞任务退出，槽位不会在取消时提前释放。
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            let mut restorer = veil::proxy::SseRestorer::new_strict(&state.store, creator);
            while let Some(job) = receiver.blocking_recv() {
                let started = Instant::now();
                let finish = job.chunk.is_none();
                let result = match job.chunk {
                    Some(chunk) => restorer.push(&chunk),
                    None => restorer.flush(),
                }
                .map_err(Error::from);
                let failed = result.is_err();
                metrics.observe(true, failed, started);
                if job.reply.send(remember(result, &errors)).is_err() || finish || failed {
                    break;
                }
            }
        });
        Ok(Stream { sender })
    }
}

fn remember<T>(result: Result<T, Error>, errors: &Mutex<Option<Failure>>) -> Result<T, Error> {
    if let Err(error) = result.as_ref() {
        let mut last = errors.lock().expect("veil error state");
        // 存储故障一旦出现即关闭入口，后续在途流的错误不能覆盖这个故障状态。
        if !matches!(
            *last,
            Some(Failure {
                error: Error::StoreUnavailable,
                ..
            })
        ) {
            *last = Some(Failure {
                error: *error,
                observed_at: SystemTime::now(),
            });
        }
    }
    result
}

struct StreamJob {
    chunk: Option<String>,
    reply: oneshot::Sender<Result<String, Error>>,
}

pub struct Stream {
    sender: mpsc::Sender<StreamJob>,
}

impl Stream {
    /// 输入必须是完整 UTF-8 SSE 事件；网络分片与原始用量观察由 Provider 负责。
    pub async fn push(&mut self, event: String) -> Result<String, Error> {
        self.submit(Some(event)).await
    }
    pub async fn finish(&mut self) -> Result<String, Error> {
        self.submit(None).await
    }
    async fn submit(&self, chunk: Option<String>) -> Result<String, Error> {
        let (reply, receiver) = oneshot::channel();
        self.sender
            .send(StreamJob { chunk, reply })
            .await
            .map_err(|_| Error::WorkerUnavailable)?;
        receiver.await.map_err(|_| Error::WorkerUnavailable)?
    }
}

#[cfg(test)]
mod tests;
