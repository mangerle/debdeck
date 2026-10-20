//! 二进制入口：crate 只暴露 [`debdeck::app::run`]，其余全部 `pub(crate)`。
//! 因此这里没有业务逻辑，转发一次即可。

fn main() -> eframe::Result<()> {
    debdeck::app::run()
}
