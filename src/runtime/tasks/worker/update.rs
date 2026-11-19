//! DebDeck 自身版本更新 worker：检查、下载验签与安装。

use std::thread::{self, JoinHandle};

use async_channel::Receiver;

use crate::backend::self_update::{
    DownloadedUpdate, UpdateEvent, UpdateInfo, UpdateStage, check_update, confirm_update_success,
};
use crate::runtime::events::{BackendEvent, EventSink, report_error};
use crate::util::format_bytes;

use super::super::{UpdateJob, Worker};

pub(in crate::runtime::tasks) fn spawn_update(
    rx: Receiver<UpdateJob>,
    worker: Worker,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let Worker { sink, .. } = worker;
        while let Ok(job) = rx.recv_blocking() {
            handle_update_job(job, &sink);
        }
    })
}

fn handle_update_job(job: UpdateJob, sink: &EventSink) {
    match job {
        UpdateJob::Check => handle_update_check(sink),
        UpdateJob::Download(update) => handle_update_download(&update, sink),
        UpdateJob::Install(downloaded) => handle_update_install(&downloaded, sink),
        UpdateJob::ConfirmSuccess => {
            if let Err(error) = confirm_update_success() {
                tracing::warn!("确认自更新成功状态失败：{error}");
            }
        }
    }
}

fn handle_update_check(sink: &EventSink) {
    let result = check_update(None).map_err(|error| report_error(&error));
    sink.send(BackendEvent::SelfUpdateChecked(result));
}

fn handle_update_download(update: &UpdateInfo, sink: &EventSink) {
    let result = update
        .download_blocking(|event| sink.send(update_progress_event(event)))
        .map_err(|error| report_error(&error));
    sink.send(BackendEvent::SelfUpdateDownloaded(result));
}

fn handle_update_install(downloaded: &DownloadedUpdate, sink: &EventSink) {
    let result = downloaded
        .install(|event| sink.send(update_progress_event(event)))
        .map_err(|error| report_error(&error));
    sink.send(BackendEvent::SelfUpdateInstalled(result));
}

/// 把自更新事件压缩成一条界面消息；下载进度直接透传百分比。
fn update_progress_event(event: UpdateEvent) -> BackendEvent {
    let (percent, message) = match event {
        UpdateEvent::DownloadProgress {
            percent,
            speed_bytes_per_sec,
            ..
        } => {
            let speed = speed_bytes_per_sec
                .map_or_else(String::new, |bytes| format!("，{}/s", format_bytes(bytes)));
            (percent, format!("正在下载并校验更新包{speed}"))
        }
        UpdateEvent::Stage(stage) => (None, update_stage_label(stage).to_string()),
        UpdateEvent::Retrying {
            attempt,
            max_retries,
            ..
        } => (
            None,
            format!("网络请求失败，正在重试（{attempt}/{max_retries}）"),
        ),
        UpdateEvent::Failed { reason } => (None, format!("更新失败：{reason}")),
    };
    BackendEvent::SelfUpdateDownloadProgress { percent, message }
}

fn update_stage_label(stage: UpdateStage) -> &'static str {
    match stage {
        UpdateStage::CheckingManifest => "正在检查更新清单",
        UpdateStage::DownloadStarted => "开始下载更新包",
        UpdateStage::VerifyingChecksum => "正在校验更新包完整性",
        UpdateStage::VerifyingSignature => "正在校验更新签名",
        UpdateStage::ExtractingArchive => "正在解压更新包",
        UpdateStage::Installing => "正在安装新版本",
        UpdateStage::ReadyToRestart => "新版本已就绪，等待重启",
        UpdateStage::DeferredToReboot => "更新已推迟到系统重启后生效",
        UpdateStage::Completed => "更新安装完成",
    }
}
