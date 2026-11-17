//! Claude 隐私存储（veil 映射卷）的备份快照适配器
//!
//! veil 在独立目录生成一致快照后打包为 `{backup_id}.veil.tar.gz`，与数据库归档共用备份暂存目录。
//! 归档不含主密钥；解包到空的 veil 数据目录并放回原主密钥即可恢复

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use gateway_admin::{
    model::backup::{BackupError, code},
    ports::backup::{DumpArtifact, DumpRequest, PrivacyStoreSnapshotPort, StagedArtifact},
};
use sha2::{Digest as _, Sha256};

/// 归档内固定的文件顺序；清单在前，便于人工检查
const ARCHIVE_ENTRIES: &[&str] = &[
    gateway_veil::SNAPSHOT_MANIFEST,
    "initialized",
    "rules-revision",
    "rules.toml",
    "mappings.db",
];

/// 基于同进程 veil 运行时的隐私存储快照
pub struct VeilBackupSnapshot {
    veil: Arc<gateway_veil::Runtime>,
    staging: PathBuf,
}

impl VeilBackupSnapshot {
    /// `staging` 必须与数据库归档的备份暂存目录相同，由 Store 负责创建与权限
    #[must_use]
    pub fn new(veil: Arc<gateway_veil::Runtime>, staging: PathBuf) -> Self {
        Self { veil, staging }
    }

    fn work_path(&self, backup_id: &str) -> PathBuf {
        self.staging.join(format!("{backup_id}.veil.d"))
    }

    fn partial_path(&self, backup_id: &str) -> PathBuf {
        self.staging.join(format!("{backup_id}.veil.partial"))
    }

    fn final_path(&self, backup_id: &str) -> PathBuf {
        self.staging.join(format!("{backup_id}.veil.tar.gz"))
    }

    fn cleanup_paths(&self, backup_id: &str) -> io::Result<()> {
        let work = remove_directory(&self.work_path(backup_id));
        let partial = remove_file(&self.partial_path(backup_id));
        let complete = remove_file(&self.final_path(backup_id));
        work.and(partial).and(complete)
    }
}

#[async_trait]
impl PrivacyStoreSnapshotPort for VeilBackupSnapshot {
    async fn snapshot(&self, request: DumpRequest) -> Result<Option<DumpArtifact>, BackupError> {
        if request.cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let work = self.work_path(&request.backup_id);
        let partial = self.partial_path(&request.backup_id);
        let final_path = self.final_path(&request.backup_id);
        // 上次中断留下的工作目录不能混入本次快照
        remove_directory(&work).map_err(|_| failed("无法清理隐私存储暂存目录"))?;
        remove_file(&partial).map_err(|_| failed("无法清理隐私存储暂存文件"))?;
        let _work_cleanup = Cleanup::Directory(work.clone());
        let manifest = tokio::select! {
            biased;
            _ = request.cancellation.cancelled() => return Err(cancelled()),
            result = self.veil.snapshot(work.clone()) => result.map_err(|error| {
                tracing::warn!(backup_id = %request.backup_id, error = %error, "veil 快照失败");
                failed("veil 映射卷不可用或快照失败")
            })?,
        };
        if manifest.is_none() {
            return Ok(None);
        }
        let _partial_cleanup = Cleanup::File(partial.clone());
        let archive = {
            let (work, partial) = (work.clone(), partial.clone());
            tokio::task::spawn_blocking(move || write_archive(&work, &partial))
        };
        let (size_bytes, sha256) = tokio::select! {
            biased;
            _ = request.cancellation.cancelled() => return Err(cancelled()),
            result = archive => result
                .map_err(|_| failed("隐私存储打包任务异常退出"))?
                .map_err(archive_failed)?,
        };
        std::fs::rename(&partial, &final_path).map_err(|_| failed("隐私存储归档改名失败"))?;
        Ok(Some(DumpArtifact {
            path: final_path,
            size_bytes,
            sha256,
        }))
    }

    async fn inspect_staging(
        &self,
        backup_id: &str,
    ) -> Result<Option<StagedArtifact>, BackupError> {
        let path = self.final_path(backup_id);
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(failed("读取隐私存储暂存信息失败")),
        }
        let hashed = path.clone();
        let (size_bytes, sha256) = tokio::task::spawn_blocking(move || hash_file(&hashed))
            .await
            .map_err(|_| failed("读取隐私存储暂存失败"))?
            .map_err(|_| failed("读取隐私存储暂存失败"))?;
        Ok(Some(StagedArtifact {
            path,
            size_bytes,
            sha256,
        }))
    }

    async fn cleanup_staging(&self, backup_id: &str) -> Result<(), BackupError> {
        self.cleanup_paths(backup_id)
            .map_err(|_| failed("清理隐私存储暂存失败"))
    }
}

/// 把快照目录打成 tar.gz，返回压缩后字节数与 sha256；文件保持 0600 权限
fn write_archive(work: &Path, partial: &Path) -> io::Result<(u64, String)> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(partial)?;
    let sink = HashingWriter {
        inner: io::BufWriter::new(file),
        size: 0,
        hash: Sha256::new(),
    };
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        sink,
        flate2::Compression::default(),
    ));
    for name in ARCHIVE_ENTRIES {
        let path = work.join(name);
        if path.try_exists()? {
            builder.append_path_with_name(&path, name)?;
        }
    }
    let sink = builder.into_inner()?.finish()?;
    let HashingWriter { inner, size, hash } = sink;
    let file = inner.into_inner().map_err(io::IntoInnerError::into_error)?;
    file.sync_all()?;
    Ok((size, hex::encode(hash.finalize())))
}

fn hash_file(path: &Path) -> io::Result<(u64, String)> {
    let mut file = std::fs::File::open(path)?;
    let mut sink = HashingWriter {
        inner: io::sink(),
        size: 0,
        hash: Sha256::new(),
    };
    io::copy(&mut file, &mut sink)?;
    Ok((sink.size, hex::encode(sink.hash.finalize())))
}

struct HashingWriter<W> {
    inner: W,
    size: u64,
    hash: Sha256,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(bytes)?;
        self.size += count as u64;
        self.hash.update(&bytes[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// 只负责中间产物；完整归档留给上传与恢复流程
enum Cleanup {
    Directory(PathBuf),
    File(PathBuf),
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let result = match self {
            Self::Directory(path) => remove_directory(path),
            Self::File(path) => remove_file(path),
        };
        if let Err(error) = result {
            tracing::warn!(error = %error, "清理隐私存储中间文件失败");
        }
    }
}

fn remove_directory(path: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn archive_failed(error: io::Error) -> BackupError {
    if error.kind() == io::ErrorKind::StorageFull {
        BackupError::new(code::STAGING_SPACE_EXHAUSTED, "暂存磁盘空间不足".to_owned())
    } else {
        failed("隐私存储打包失败")
    }
}

fn failed(message: &'static str) -> BackupError {
    BackupError::new(code::PRIVACY_STORE_FAILED, message.to_owned())
}

fn cancelled() -> BackupError {
    BackupError::new(code::CANCELLED, "备份导出已取消".to_owned())
}
