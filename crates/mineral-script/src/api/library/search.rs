//! 歌曲搜索句柄：每次 next 沿用来源进度，close 或回收时释放读取。

use mineral_model::SourceKind;
use mlua::{Function, Lua, Table, UserData, UserDataMethods};
use tokio::sync::{mpsc, oneshot};

use crate::host::ScriptHost;
use crate::message::{QueryId, ScriptCmd};

/// Lua 持有请求发送端，daemon 持有来源流；每批回调复用一次性查询注册表。
struct Search {
    /// 关闭后不能继续提交。
    requests: Option<mpsc::UnboundedSender<QueryId>>,

    /// 丢弃发送端通知 daemon 取消正在读取的批次。
    closed: Option<oneshot::Sender<()>>,

    /// 为每次 next 登记回调。
    host: ScriptHost,
}

impl UserData for Search {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("next", |lua, search, callback: Function| {
            let requests = search
                .requests
                .as_ref()
                .ok_or_else(|| mlua::Error::runtime("search is closed"))?;
            let query = search.host.register_query(lua, callback)?;
            if let Err(error) = requests.send(query) {
                search.host.pending.lock().take(query);
                return Err(mlua::Error::external(error));
            }
            Ok(())
        });
        methods.add_method_mut("close", |_lua, search, ()| {
            search.requests.take();
            search.closed.take();
            Ok(())
        });
    }
}

pub(crate) fn install(lua: &Lua, library: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let host = host.clone();
    library.set(
        "search",
        lua.create_function(move |lua, (term, opts): (String, Option<Table>)| {
            let source = opts
                .as_ref()
                .map(|opts| opts.get::<Option<String>>("source"))
                .transpose()?
                .flatten()
                .map(|name| SourceKind::from_name(&name));
            let (requests, receiver) = mpsc::unbounded_channel();
            let (closed, closing) = oneshot::channel();
            host.commands
                .send(ScriptCmd::LibrarySearch {
                    term,
                    source,
                    requests: receiver,
                    closed: closing,
                })
                .map_err(mlua::Error::external)?;
            lua.create_userdata(Search {
                requests: Some(requests),
                closed: Some(closed),
                host: host.clone(),
            })
        })?,
    )
}
