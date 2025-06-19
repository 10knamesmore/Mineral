use ratatui_image::{picker::Picker, protocol::StatefulProtocol};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::{self},
};
use std::{io::Cursor, path::Path};
use tokio::{
    fs,
    sync::mpsc::{self},
};

enum ImageCacheType {
    Playlist,
    Album,
    Artist,
}

pub enum ImageState {
    NotRequested,
    Loading,
    Loaded(RefCell<StatefulProtocol>),
    Failed(String),
}

pub struct ImageLoadRequest {
    image_type: ImageCacheType,
    id: u64,
}

struct ImageloadResult {
    image_type: ImageCacheType,
    id: u64,
    result: Result<StatefulProtocol, String>,
}

/// 图片缩略图缓存
pub(crate) struct RenderCache {
    not_requested_placeholder: ImageState,
    playlist_cover: HashMap<u64, ImageState>,
    album_cover: HashMap<u64, ImageState>,
    artist_cover: HashMap<u64, ImageState>,

    load_request_sender: mpsc::UnboundedSender<ImageLoadRequest>,
    load_result_receiver: mpsc::UnboundedReceiver<ImageloadResult>,
}

impl RenderCache {
    pub fn new(picker: Picker, cache_path: String) -> Self {
        let (load_request_sender, load_request_receiver) = mpsc::unbounded_channel();
        let (load_result_sender, load_result_receiver) = mpsc::unbounded_channel();

        let cache_path_cloned = cache_path.clone();

        tokio::spawn(async move {
            Self::image_loader_task(
                load_request_receiver,
                load_result_sender,
                cache_path_cloned,
                picker,
            )
            .await;
        });

        Self {
            not_requested_placeholder: ImageState::NotRequested,
            playlist_cover: HashMap::new(),
            album_cover: HashMap::new(),
            artist_cover: HashMap::new(),
            load_request_sender,
            load_result_receiver,
        }
    }

    pub(crate) fn not_requested(&mut self) -> &ImageState {
        &mut self.not_requested_placeholder
    }

    pub(crate) fn playlist_cover(&mut self, id: u64) -> &ImageState {
        self.poll_image_results();

        if let std::collections::hash_map::Entry::Vacant(entry) = self.playlist_cover.entry(id) {
            entry.insert(ImageState::Loading);
            self.request_image_load(ImageCacheType::Playlist, id);
        }

        self.playlist_cover.get(&id).unwrap()
    }
    pub(crate) fn artist_cover(&mut self, id: u64) -> &ImageState {
        self.poll_image_results();

        if let std::collections::hash_map::Entry::Vacant(entry) = self.artist_cover.entry(id) {
            entry.insert(ImageState::Loading);
            self.request_image_load(ImageCacheType::Artist, id);
        }

        self.artist_cover.get(&id).unwrap()
    }
    pub(crate) fn album_cover(&mut self, id: u64) -> &ImageState {
        self.poll_image_results();

        if let std::collections::hash_map::Entry::Vacant(entry) = self.album_cover.entry(id) {
            entry.insert(ImageState::Loading);
            self.request_image_load(ImageCacheType::Album, id);
        }

        self.album_cover.get(&id).unwrap()
    }

    fn request_image_load(&self, image_type: ImageCacheType, id: u64) {
        let request = ImageLoadRequest { image_type, id };

        if let Err(e) = self.load_request_sender.send(request) {
            eprintln!("Failed to send image load request: {}", e)
        }
    }

    // 轮询, 更新load结果到self
    fn poll_image_results(&mut self) {
        while let Ok(result) = self.load_result_receiver.try_recv() {
            let state = match result.result {
                Ok(image) => ImageState::Loaded(RefCell::new(image)),
                Err(e) => ImageState::Failed(e),
            };

            match result.image_type {
                ImageCacheType::Playlist => {
                    self.playlist_cover.insert(result.id, state);
                }
                ImageCacheType::Album => {
                    self.album_cover.insert(result.id, state);
                }
                ImageCacheType::Artist => {
                    self.artist_cover.insert(result.id, state);
                }
            }
        }
    }

    // 等待接收加载图片的请求
    async fn image_loader_task(
        mut request_receiver: mpsc::UnboundedReceiver<ImageLoadRequest>,
        result_sender: mpsc::UnboundedSender<ImageloadResult>,
        cache_path: String,
        picker: Picker,
    ) {
        while let Some(request) = request_receiver.recv().await {
            let result = Self::load_image_async(&cache_path, &picker, &request).await;

            let load_result = ImageloadResult {
                image_type: request.image_type,
                id: request.id,
                result,
            };

            if let Err(e) = result_sender.send(load_result) {
                eprintln!("Failed to send image load result: {}", e);
                break;
            }
        }
    }

    // 异步加载图片
    async fn load_image_async(
        cache_path: &str,
        picker: &Picker,
        request: &ImageLoadRequest,
    ) -> Result<StatefulProtocol, String> {
        let (image_type, id) = (&request.image_type, request.id);

        match Self::try_path_from_disk(cache_path, image_type, id).await {
            Ok(file_path_opt) => match file_path_opt {
                Some(file_path) => {
                    let data = tokio::fs::read(&file_path)
                        .await
                        .map_err(|e| e.to_string())?;

                    let format = image::guess_format(&data)
                        .map_err(|e| format!("无法识别 {} 文件格式 {}", &file_path, e))?;
                    let cursor = Cursor::new(data);

                    let decoded_image = image::ImageReader::with_format(cursor, format)
                        .decode()
                        .map_err(|e| format!("文件 {} 解码时发生错误 {}", &file_path, e))?;

                    let image = picker.new_resize_protocol(decoded_image);
                    Ok(image)
                }
                None => match Self::try_from_net(cache_path, picker, image_type, id).await {
                    Ok(_) => todo!("从net获取图片尚未实现"),
                    Err(_) => todo!("从net获取图片尚未实现"),
                },
            },
            Err(e) => Err(format!("读取图片的时候发生IO错误: {}", e)),
        }
    }

    async fn try_from_net(
        cache_path: &str,
        picker: &Picker,
        image_type: &ImageCacheType,
        id: u64,
    ) -> io::Result<StatefulProtocol> {
        // TODO: 尝试通过api获取网络图片,并保存到本地,直接返回StatefulProtocol
        todo!("从net获取图片尚未实现")
    }

    /// 尝试查找type为image_type的图片在磁盘上是否存在, 如果存在, 返回图片的路径
    ///
    /// # 参数
    /// - `image_type`: 图片类型
    /// - `id`: image_type 对应类型的 ID
    ///
    /// # 返回
    /// - `Err(e)`: 发生io错误
    /// - `Option<String>`: 返回图片的路径，如果不存在则返回 None
    async fn try_path_from_disk(
        cache_path: &str,
        image_type: &ImageCacheType,
        id: u64,
    ) -> io::Result<Option<String>> {
        // TODO: 对非UTF-8编码的文件系统的支持
        let subdir = match image_type {
            ImageCacheType::Playlist => "playlist",
            ImageCacheType::Album => "album",
            ImageCacheType::Artist => "artist",
        };

        let dir_path = Path::new(cache_path).join("images").join(subdir);

        let prefix = format!("{}.", id);
        let mut rd = fs::read_dir(&dir_path).await?;

        while let Some(entry) = rd.next_entry().await? {
            // 获取文件名，并检查是否为有效 UTF-8
            let file_name_os = entry.file_name();

            if let Some(file_name_str) = file_name_os.to_str() {
                if file_name_str.starts_with(&prefix) {
                    let full_path = entry.path();

                    // 检查整个文件路径是否为有效UTF-8
                    if let Some(path_str) = full_path.to_str() {
                        return Ok(Some(path_str.to_string()));
                    } else {
                        // 文件路径不是有效 UTF-8 字符串
                        continue;
                    }
                }
            }
        }

        Ok(None)
    }
}
