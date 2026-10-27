// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accounts;
mod config;
mod image_util;
mod msa;
mod preview3d;
mod skins;

use image::RgbaImage;
use neutrino_minecraft_launcher::{core, instances, java};
use slint::{Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

mod desktop;

slint::include_modules!();

/// 默认皮肤：和 Slint 里用到的头像来自同一份 64×64 皮肤文件。
const DEFAULT_STEVE_SKIN: &[u8] = include_bytes!("../ui/images/faces/steve.png");
const DEFAULT_ALEX_SKIN: &[u8] = include_bytes!("../ui/images/faces/alex.png");

fn main() -> Result<(), Box<dyn Error>> {
    // 先取外部环境变量，再加载 .env：CurseForge Key 里可能有 `$`，不能交给 dotenvy 展开。
    let external_curseforge_key = std::env::var(core::curseforge::API_KEY_ENVS[0])
        .ok()
        .or_else(|| std::env::var(core::curseforge::API_KEY_ENVS[1]).ok());
    let env_path = dotenvy::dotenv().ok();
    desktop::init_curseforge_key(external_curseforge_key.as_deref(), env_path.as_deref());

    // 3D 预览需要 Slint 走 wgpu 渲染。先自己建好 instance / adapter / device，
    // 失败就退回默认渲染器（预览框留空，其它功能不受影响）。
    let gpu = match select_wgpu_backend() {
        Some(gpu) => Some(gpu),
        None => {
            eprintln!("警告：wgpu 后端不可用，3D 预览已禁用");
            let _ = slint::BackendSelector::new().select();
            None
        }
    };

    // wgpu 30 在 Windows 上销毁 Queue 时会访问已经结束的线程本地状态，退出时 panic。
    // 留一份引用到最后故意泄漏，让 Queue / Device 在进程结束时不被真正 drop。
    let gpu_guard = gpu
        .as_ref()
        .map(|gpu| (gpu.device.clone(), gpu.queue.clone()));

    let ui = AppWindow::new()?;

    // 外观（主题 / 背景）来自 conf.toml；UI 里切换主题时再写回同一个文件。
    let config_store = Rc::new(RefCell::new(config::Store::load_default()));
    let wallpaper_cache = Arc::new(WallpaperThumbnailCache::default());
    apply_config(&ui, config_store.borrow().config(), &wallpaper_cache);
    let background = BackgroundUi {
        config: config_store.clone(),
        cache: wallpaper_cache.clone(),
        ui: ui.as_weak(),
    };

    let game_dir = core::default_game_dir();
    let game_dir_text = game_dir.display().to_string();
    ui.global::<AppState>()
        .set_game_dir(game_dir_text.clone().into());

    let instances = instances::detect_instances(&game_dir_text)
        .into_iter()
        .map(|instance| InstanceInfo {
            name: instance.name.into(),
            path: instance.path.into(),
        })
        .collect::<Vec<_>>();

    ui.global::<AppState>()
        .set_instances(ModelRc::new(VecModel::from(instances)));

    let account_store = Arc::new(Mutex::new(accounts::Store::load_default()));
    update_account_state(&ui, &lock_store(&account_store));

    // ---- 3D 预览：wgpu 离屏渲染到纹理，再导入成 Slint 的 image ----
    if let Some(gpu) = gpu {
        let preview = Arc::new(Mutex::new(preview3d::Preview3d::new(gpu.device, gpu.queue)));
        ui.global::<AppState>().set_preview_ready(true);

        let drag_state = preview.clone();
        ui.global::<Actions>().on_preview_orbit_start(move || {
            if let Ok(mut scene) = drag_state.lock() {
                scene.begin_drag();
            }
        });

        let drag_state = preview.clone();
        let preview_handle = ui.as_weak();
        ui.global::<Actions>().on_preview_orbit(move |dx, dy| {
            let Ok(mut scene) = drag_state.lock() else {
                return;
            };
            scene.drag_to(dx, dy);
            scene.render();

            if let (Some(image), Some(ui)) = (scene.image(), preview_handle.upgrade()) {
                ui.global::<AppState>().set_preview_image(image);
            }
        });

        let zoom_state = preview.clone();
        let zoom_handle = ui.as_weak();
        ui.global::<Actions>().on_zoom_preview(move |delta| {
            let Ok(mut scene) = zoom_state.lock() else {
                return;
            };
            scene.zoom_by(delta);
            scene.render();

            if let (Some(image), Some(ui)) = (scene.image(), zoom_handle.upgrade()) {
                ui.global::<AppState>().set_preview_image(image);
            }
        });

        let skin_state = preview.clone();
        let skin_handle = ui.as_weak();
        let skin_store = account_store.clone();
        ui.global::<Actions>().on_preview_skin(move |name, light| {
            let Some((skin_bytes, cape_bytes, cape_enabled)) = ({
                let store = lock_store(&skin_store);
                preview_assets(&store, name.as_str())
            }) else {
                return;
            };

            let Ok(skin) = image::load_from_memory(&skin_bytes) else {
                return;
            };
            let skin = skin.to_rgba8();
            let cape = cape_bytes.and_then(|bytes| {
                image::load_from_memory(&bytes)
                    .ok()
                    .map(|image| image.to_rgba8())
            });

            let Ok(mut scene) = skin_state.lock() else {
                return;
            };
            scene.set_light_background(light);
            if !scene.set_skin(&skin) {
                return;
            }
            match cape {
                Some(cape) if scene.set_cape(&cape) => {
                    scene.set_cape_visible(cape_enabled);
                }
                _ => scene.clear_cape(),
            }
            scene.render();

            if let (Some(image), Some(ui)) = (scene.image(), skin_handle.upgrade()) {
                ui.global::<AppState>().set_preview_image(image);
            }
        });
    }

    if let Some(info) = java::detect_java() {
        ui.global::<AppState>().set_java_info(JavaInfo {
            version: info.version.into(),
            major: info.major,
            lts: info.lts,
            vendor: info.vendor.into(),
            path: info.path.into(),
        });
    }

    ui.global::<Actions>().on_open_folder(|path| {
        if let Err(err) = open_folder(path.as_str()) {
            eprintln!("Failed to open folder: {err}");
        }
    });

    ui.global::<Actions>().on_open_url(|url| {
        if let Err(err) = open_url(url.as_str()) {
            eprintln!("Failed to open URL: {err}");
        }
    });

    let saved_config = config_store.clone();
    ui.global::<Actions>()
        .on_save_theme_preference(move |mode| {
            let mode = config::ColorMode::parse(mode.as_str());
            if let Err(error) = saved_config.borrow_mut().set_color_mode(mode) {
                eprintln!("保存主题设置失败：{error}");
            }
        });

    let choose_wallpaper = background.clone();
    ui.global::<Actions>().on_choose_custom_wallpaper(move || {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("图片", &["png", "jpg", "jpeg"])
            .set_title("选择壁纸（可多选）")
            .pick_files()
        else {
            return "".into();
        };

        let usable = paths
            .into_iter()
            .filter(|path| wallpaper_is_readable(path))
            .collect::<Vec<_>>();
        if usable.is_empty() {
            return "所选文件都不是可读取的 PNG / JPG 图片".into();
        }

        choose_wallpaper.update(|store| store.add_custom_wallpapers(&usable))
    });

    let choose_mode = background.clone();
    ui.global::<Actions>()
        .on_choose_background_mode(move |mode| {
            choose_mode.update(|store| {
                store.set_background_mode(config::BackgroundMode::parse(mode.as_str()))
            })
        });

    let set_color = background.clone();
    ui.global::<Actions>().on_set_wallpaper_color(move |value| {
        set_color.update(|store| store.set_background_color(value.as_str()))
    });

    let pick_color = background.clone();
    ui.global::<Actions>()
        .on_pick_wallpaper_color(move |color| {
            let hex = config::hex_color(color.red(), color.green(), color.blue());
            pick_color.update(move |store| store.set_background_color(&hex))
        });

    let select_wallpaper_action = background.clone();
    ui.global::<Actions>().on_select_wallpaper(move |path| {
        let path = path.to_string();
        if !wallpaper_is_readable(Path::new(&path)) {
            return "无法读取这张壁纸".into();
        }

        select_wallpaper_action.update(|store| store.select_custom_wallpaper(&path))
    });

    let remove_wallpaper = background.clone();
    ui.global::<Actions>().on_remove_wallpaper(move |path| {
        remove_wallpaper.update(|store| store.remove_custom_wallpaper(path.as_str()))
    });

    let random_wallpaper = background.clone();
    ui.global::<Actions>()
        .on_set_wallpaper_random(move |random| {
            if random && !random_wallpaper.has_wallpaper_file() {
                return "请先添加至少一张可读取的壁纸".into();
            }

            random_wallpaper.update(move |store| store.set_random_background(random))
        });

    let reset_wallpaper = background.clone();
    ui.global::<Actions>()
        .on_reset_wallpaper(move || reset_wallpaper.update(|store| store.reset_background()));

    let launch_handle = ui.as_weak();
    let launch_store = account_store.clone();
    ui.global::<Actions>().on_launch_instance(move |name| {
        match launch_game(&launch_store, name.as_str()) {
            Ok((message, mut child)) => {
                if let Some(ui) = launch_handle.upgrade() {
                    let state = ui.global::<AppState>();
                    state.set_launch_ok(true);
                    state.set_launch_message(message.into());
                }

                // 后台等待游戏退出，退出后清掉“已启动”提示。
                let weak = launch_handle.clone();
                std::thread::spawn(move || {
                    let _ = child.wait();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = weak.upgrade() {
                            let state = ui.global::<AppState>();
                            state.set_launch_ok(false);
                            state.set_launch_message("".into());
                        }
                    });
                });
            }
            Err(error) => {
                if let Some(ui) = launch_handle.upgrade() {
                    let state = ui.global::<AppState>();
                    state.set_launch_ok(false);
                    state.set_launch_message(error.into());
                }
            }
        }
    });

    // 实例内容（模组 / 资源包 / 光影）全部在后台线程扫描和解码，
    // UI 线程只负责把结果换成模型，点击实例不会再卡住窗口。
    let content_loader = Arc::new(InstanceContentLoader::default());
    ui.global::<Actions>().on_load_instance_content({
        let loader = content_loader.clone();
        let weak = ui.as_weak();
        move |path| loader.load(&weak, path.as_str())
    });

    ui.global::<Actions>().on_toggle_content({
        let loader = content_loader.clone();
        let weak = ui.as_weak();
        move |path, folder, name| {
            let loader = loader.clone();
            let weak = weak.clone();
            let path = path.to_string();
            let folder = folder.to_string();
            let name = name.to_string();

            // 重命名很快，但仍然放后台，避免网络盘 / 机械盘上卡住 UI。
            std::thread::spawn(move || {
                if let Err(error) = instances::toggle_content(&path, &folder, &name) {
                    eprintln!("切换内容状态失败：{error}");
                }

                let reload_loader = loader.clone();
                let reload_weak = weak.clone();
                let reload_path = path.clone();
                let _ = weak.upgrade_in_event_loop(move |_| {
                    reload_loader.reload_if_current(&reload_weak, &reload_path);
                });
            });
        }
    });

    ui.global::<Actions>().on_delete_content({
        let loader = content_loader.clone();
        let weak = ui.as_weak();
        move |path, folder, name| {
            let loader = loader.clone();
            let weak = weak.clone();
            let path = path.to_string();
            let folder = folder.to_string();
            let name = name.to_string();

            // 删除文件夹可能比较慢，和重命名一样放到后台，完成后重新扫描。
            std::thread::spawn(move || {
                match instances::delete_content(&path, &folder, &name) {
                    Ok(true) => instances::forget_tracked_content(&path, &folder, &name),
                    Ok(false) => eprintln!("删除内容失败：文件不存在（{folder}/{name}）"),
                    Err(error) => eprintln!("删除内容失败：{error}"),
                }

                let reload_loader = loader.clone();
                let reload_weak = weak.clone();
                let reload_path = path.clone();
                let _ = weak.upgrade_in_event_loop(move |_| {
                    reload_loader.reload_if_current(&reload_weak, &reload_path);
                });
            });
        }
    });

    let add_handle = ui.as_weak();
    let add_store = account_store.clone();
    ui.global::<Actions>().on_add_offline_account(move |name| {
        let mut store = lock_store(&add_store);
        match store.add_offline(name.as_str()) {
            Ok(()) => {
                if let Some(ui) = add_handle.upgrade() {
                    update_account_state(&ui, &store);
                }
                "".into()
            }
            Err(error) => error.into(),
        }
    });

    let remove_handle = ui.as_weak();
    let remove_store = account_store.clone();
    ui.global::<Actions>().on_remove_account(move |name| {
        let mut store = lock_store(&remove_store);
        if store.remove(name.as_str()).is_ok()
            && let Some(ui) = remove_handle.upgrade()
        {
            update_account_state(&ui, &store);
        }
    });

    let active_handle = ui.as_weak();
    let active_store = account_store.clone();
    ui.global::<Actions>().on_set_active_account(move |name| {
        let mut store = lock_store(&active_store);
        if store.set_active(name.as_str()).is_ok()
            && let Some(ui) = active_handle.upgrade()
        {
            update_account_state(&ui, &store);
        }
    });

    let skin_handle = ui.as_weak();
    let skin_store = account_store.clone();
    ui.global::<Actions>()
        .on_set_default_skin(move |name, skin| {
            let skin = match skin {
                SkinType::Steve => accounts::Skin::Steve,
                SkinType::Alex => accounts::Skin::Alex,
                SkinType::Custom => return,
            };

            let mut store = lock_store(&skin_store);
            if store.set_skin(name.as_str(), skin).is_ok()
                && let Some(ui) = skin_handle.upgrade()
            {
                update_account_state(&ui, &store);
            }
        });

    let custom_handle = ui.as_weak();
    let custom_store = account_store.clone();
    ui.global::<Actions>().on_choose_custom_skin(move |name| {
        match accounts::pick_custom_skin(name.as_str()) {
            Ok(Some(skin)) => {
                let mut store = lock_store(&custom_store);
                if store.set_skin(name.as_str(), skin).is_ok()
                    && let Some(ui) = custom_handle.upgrade()
                {
                    update_account_state(&ui, &store);
                }
            }
            Ok(None) => {}
            Err(error) => eprintln!("选择自定义皮肤失败：{error}"),
        }
    });

    let cape_handle = ui.as_weak();
    let cape_store = account_store.clone();
    ui.global::<Actions>().on_choose_custom_cape(move |name| {
        match accounts::pick_custom_cape(name.as_str()) {
            Ok(Some(path)) => {
                let mut store = lock_store(&cape_store);
                if let Err(error) = store.set_cape(name.as_str(), Some(path)) {
                    return error.into();
                }
                if let Some(ui) = cape_handle.upgrade() {
                    update_account_state(&ui, &store);
                }
                "".into()
            }
            Ok(None) => "".into(),
            Err(error) => error.into(),
        }
    });

    let cape_handle = ui.as_weak();
    let cape_store = account_store.clone();
    ui.global::<Actions>()
        .on_set_cape_enabled(move |name, enabled| {
            let mut store = lock_store(&cape_store);
            if let Err(error) = store.set_cape_enabled(name.as_str(), enabled) {
                eprintln!("切换披风渲染失败：{error}");
                return;
            }
            if let Some(ui) = cape_handle.upgrade() {
                update_account_state(&ui, &store);
            }
        });

    let ui_handle = ui.as_weak();
    ui.global::<Actions>().on_start_microsoft_login(move || {
        let weak = ui_handle.clone();
        let login_store = account_store.clone();

        if let Some(ui) = weak.upgrade() {
            let status = ui.global::<MsaState>().get_status();
            if status == MsaStatus::Starting || status == MsaStatus::Waiting {
                return;
            }

            let state = ui.global::<MsaState>();
            state.set_status(MsaStatus::Starting);
            state.set_error("".into());
        } else {
            return;
        }

        let client_id = match msa::client_id() {
            Ok(client_id) => client_id,
            Err(error) => {
                if let Some(ui) = weak.upgrade() {
                    let state = ui.global::<MsaState>();
                    state.set_status(MsaStatus::MissingClientId);
                    state.set_error(error.into());
                }
                return;
            }
        };

        std::thread::spawn(move || {
            let weak = weak.clone();
            let login_store = login_store.clone();
            let _ = msa::login(client_id, move |event| {
                let weak = weak.clone();
                let login_store = login_store.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak.upgrade() else {
                        return;
                    };

                    let state = ui.global::<MsaState>();
                    match event {
                        msa::LoginEvent::DeviceCode {
                            verification_uri,
                            user_code,
                        } => {
                            state.set_status(MsaStatus::Waiting);
                            state.set_verification_uri(verification_uri.into());
                            state.set_user_code(user_code.into());
                        }
                        msa::LoginEvent::Success(session) => {
                            let mut store = lock_store(&login_store);
                            match store.upsert_microsoft(
                                &session.player_name,
                                &session.uuid,
                                &session.access_token,
                                &session.refresh_token,
                                session.expires_at,
                            ) {
                                Ok(()) => {
                                    update_account_state(&ui, &store);
                                    state.set_status(MsaStatus::Success);
                                    state.set_player_name(session.player_name.into());
                                }
                                Err(error) => {
                                    state.set_status(MsaStatus::Error);
                                    state.set_error(error.into());
                                }
                            }
                        }
                        msa::LoginEvent::Error(error) => {
                            state.set_status(MsaStatus::Error);
                            state.set_error(error.into());
                        }
                    }
                });
            });
        });
    });

    // 发现页：版本列表与安装
    let controller = desktop::InstallController::new(ui.as_weak());

    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_refresh_versions(move || controller.refresh_versions());
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_filter_versions(move |key| controller.apply_filter(key.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_reset_add_instance(move || controller.reset_add_instance());
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_start_new_instance(move || controller.start_new_instance());
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_choose_version(move |version| controller.choose_version(version.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_choose_loader(move |key| controller.choose_loader(key.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_choose_loader_version(move |version| {
                controller.choose_loader_version(version.as_str())
            });
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_go_back(move || controller.go_back());
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_create_instance(move |name| controller.create_instance(name.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_refresh_instance_loader_versions(move |game_version, loader| {
                controller.refresh_instance_loader_versions(game_version.as_str(), loader.as_str())
            });
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>().on_switch_instance_loader(
            move |instance, game_version, loader, version| {
                controller.switch_instance_loader(
                    instance.as_str(),
                    game_version.as_str(),
                    loader.as_str(),
                    version.as_str(),
                )
            },
        );
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_open_category(move |key| controller.open_category(key.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_open_instance_download(move |kind, game, loader| {
                controller.open_instance_download(kind.as_str(), game.as_str(), loader.as_str());
            });
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_set_download_target(move |path| controller.set_download_target(path.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_check_content_updates(move |instance, folder, kind| {
                controller.check_content_updates(instance.as_str(), folder.as_str(), kind.as_str());
            });
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_update_content(move |key| controller.apply_content_update(key.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_search_projects(move |query| controller.set_query(query.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_choose_source(move |key| controller.choose_source(key.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_choose_game_version(move |version| {
                controller.choose_game_version(version.as_str())
            });
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_choose_loader_filter(move |label| controller.choose_loader_filter(label.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_choose_category_filter(move |label| {
                controller.choose_category_filter(label.as_str())
            });
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_choose_sort(move |label| controller.choose_sort(label.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_open_project(move |id| controller.open_project(id.as_str()));
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_close_project(move || controller.close_project());
    }
    {
        let controller = controller.clone();
        ui.global::<Actions>()
            .on_download_version(move |key| controller.download_version(key.as_str()));
    }

    // 启动时后台拉一次版本清单，供“实例页 → 添加实例”使用
    controller.refresh_versions();

    // 发现页默认展示模组列表，启动时也预取一次
    controller.open_category("mod");

    ui.run()?;

    // 故意泄漏，见上面 gpu_guard 的说明：避免 wgpu 在 TLS 析构阶段 drop Queue 时 panic。
    if let Some(gpu_guard) = gpu_guard {
        std::mem::forget(gpu_guard);
    }

    Ok(())
}

/// 设置页壁纸缩略图缓存；按路径 + 大小 + 修改时间判断是否还有效。
#[derive(Default)]
struct WallpaperThumbnailCache {
    entries: Mutex<HashMap<PathBuf, CachedWallpaperThumbnail>>,
}

struct CachedWallpaperThumbnail {
    len: u64,
    modified: Option<SystemTime>,
    thumbnail: Option<Arc<RgbaImage>>,
}

impl WallpaperThumbnailCache {
    fn thumbnail(&self, path: &Path) -> Option<Arc<RgbaImage>> {
        let metadata = std::fs::metadata(path).ok()?;
        let len = metadata.len();
        let modified = metadata.modified().ok();

        if let Ok(cache) = self.entries.lock()
            && let Some(hit) = cache.get(path)
            && hit.len == len
            && hit.modified == modified
        {
            return hit.thumbnail.clone();
        }

        let thumbnail = read_wallpaper_thumbnail(path).map(Arc::new);
        if let Ok(mut cache) = self.entries.lock() {
            cache.insert(
                path.to_path_buf(),
                CachedWallpaperThumbnail {
                    len,
                    modified,
                    thumbnail: thumbnail.clone(),
                },
            );
        }
        thumbnail
    }
}

/// 把壁纸 / 背景设置的回调收敛成一处：改配置、保存、再把结果推回 UI。
#[derive(Clone)]
struct BackgroundUi {
    config: Rc<RefCell<config::Store>>,
    cache: Arc<WallpaperThumbnailCache>,
    ui: slint::Weak<AppWindow>,
}

impl BackgroundUi {
    /// 返回给 Slint 的错误文本；空字符串表示成功。
    fn update(
        &self,
        apply: impl FnOnce(&mut config::Store) -> Result<(), String>,
    ) -> slint::SharedString {
        let result = {
            let mut store = self.config.borrow_mut();
            apply(&mut store)
        };
        if let Err(error) = result {
            return error.into();
        }

        if let Some(ui) = self.ui.upgrade() {
            let background = self.config.borrow().config().appearance.background.clone();
            refresh_background(&ui, &background, &self.cache);
        }
        "".into()
    }

    fn has_wallpaper_file(&self) -> bool {
        self.config
            .borrow()
            .config()
            .appearance
            .background
            .wallpaper_paths()
            .iter()
            .any(|path| path.is_file())
    }
}

/// 把 `conf.toml` 里的外观设置应用到 UI：主题、调色板、背景壁纸。
fn apply_config(ui: &AppWindow, config: &config::Config, cache: &WallpaperThumbnailCache) {
    let mode = config.appearance.theme.color_mode;
    ui.global::<Theme>().set_preference(match mode {
        config::ColorMode::Light => ThemePreference::Light,
        config::ColorMode::Dark => ThemePreference::Dark,
        config::ColorMode::System => ThemePreference::System,
    });
    ui.global::<Theme>().invoke_apply();

    apply_background(ui, &config.appearance.background);
    sync_wallpaper_state(ui, &config.appearance.background, cache);
}

/// 按配置加载背景：纯色优先，其次随机图片、`%file:<path>`，最后回退到内置 Jokull。
fn apply_background(ui: &AppWindow, background: &config::BackgroundConfig) {
    if background.mode == config::BackgroundMode::Color
        && let Some((red, green, blue)) = config::parse_hex_color(&background.color)
    {
        set_wallpaper_color(ui, red, green, blue, &background.color);
        return;
    }

    if background.random {
        let paths = background
            .wallpaper_paths()
            .into_iter()
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();

        if !paths.is_empty() {
            let path = paths[random_index(paths.len())];
            if let Some((name, image)) = read_wallpaper(path) {
                set_wallpaper(ui, name, &image);
                return;
            }
        }
    }

    if let Some(path) = background.custom_path() {
        match read_wallpaper(path) {
            Some((name, image)) => {
                set_wallpaper(ui, name, &image);
                return;
            }
            None => eprintln!("无法读取自定义壁纸 {}，回退到内置 Jokull", path.display()),
        }
    }

    let requested = background.builtin_name();
    let (name, bytes) = match requested.and_then(builtin_background) {
        Some(background) => background,
        None => {
            if let Some(requested) = requested {
                eprintln!("未知的内置背景 {requested:?}，回退到 Jokull");
            }
            builtin_background("Jokull").expect("内置壁纸 Jokull 必须存在")
        }
    };

    if let Some(image) = image_util::decode_rgba(bytes) {
        set_wallpaper(ui, format!("{name}.jpg"), &image);
    }
}

/// 配置改动后同时刷新背景和设置页里的壁纸列表。
fn refresh_background(
    ui: &AppWindow,
    background: &config::BackgroundConfig,
    cache: &WallpaperThumbnailCache,
) {
    apply_background(ui, background);
    sync_wallpaper_state(ui, background, cache);
}

/// 把已添加的壁纸（含缩略图 / 当前选中状态）推给 UI。
fn sync_wallpaper_state(
    ui: &AppWindow,
    background: &config::BackgroundConfig,
    cache: &WallpaperThumbnailCache,
) {
    let current = background.custom_path();
    let rows = background
        .wallpaper_paths()
        .into_iter()
        .map(|path| {
            let thumbnail = cache.thumbnail(path);
            let has_thumbnail = thumbnail.is_some();
            let thumbnail = thumbnail
                .map(|image| image_util::from_rgba(&image))
                .unwrap_or_default();

            WallpaperInfo {
                path: path.display().to_string().into(),
                name: wallpaper_name(path).into(),
                thumbnail,
                has_thumbnail,
                selected: !background.random && current == Some(path),
            }
        })
        .collect::<Vec<_>>();

    let state = ui.global::<AppState>();
    state.set_wallpapers(ModelRc::new(VecModel::from(rows)));
    state.set_wallpaper_random(background.random);
}

/// 读取并解码自定义壁纸，返回显示名（文件名）和 RGBA 图。
fn read_wallpaper(path: &Path) -> Option<(String, RgbaImage)> {
    let bytes = std::fs::read(path).ok()?;
    let image = image_util::decode_rgba(&bytes)?;
    Some((wallpaper_name(path), image))
}

/// 只读文件头判断图片是否可解码，选择多张图时避免把无效文件写进配置。
fn wallpaper_is_readable(path: &Path) -> bool {
    let Ok(reader) = image::ImageReader::open(path) else {
        return false;
    };
    let Ok(reader) = reader.with_guessed_format() else {
        return false;
    };
    reader.into_dimensions().is_ok()
}

/// 生成设置页列表用的小缩略图，避免把整张 4K 图塞进模型。
fn read_wallpaper_thumbnail(path: &Path) -> Option<RgbaImage> {
    const THUMB_WIDTH: u32 = 192;
    const THUMB_HEIGHT: u32 = 108;

    let bytes = std::fs::read(path).ok()?;
    let image = image_util::decode_rgba(&bytes)?;
    Some(image::imageops::thumbnail(
        &image,
        THUMB_WIDTH,
        THUMB_HEIGHT,
    ))
}

fn wallpaper_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// 没有引入 rand 依赖，用时间 + 进程号做一个一次性的伪随机下标。
fn random_index(len: usize) -> usize {
    if len <= 1 {
        return 0;
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default();
    let mut value = nanos ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    (value as usize) % len
}

fn set_wallpaper(ui: &AppWindow, name: String, image: &RgbaImage) {
    let state = ui.global::<AppState>();
    state.set_background_is_color(false);
    state.set_wallpaper(image_util::from_rgba(image));
    state.set_background_name(name.into());
}

fn set_wallpaper_color(ui: &AppWindow, red: u8, green: u8, blue: u8, hex: &str) {
    let state = ui.global::<AppState>();
    state.set_background_is_color(true);
    state.set_wallpaper_color(slint::Color::from_rgb_u8(red, green, blue));
    state.set_wallpaper_color_hex(hex.into());
    state.set_background_name(hex.into());
}

/// 内置壁纸表；名称大小写不敏感。
fn builtin_background(name: &str) -> Option<(&'static str, &'static [u8])> {
    const BACKGROUNDS: &[(&str, &[u8])] = &[(
        "Jokull",
        include_bytes!("../ui/images/backgrounds/Jokull.jpg"),
    )];

    BACKGROUNDS
        .iter()
        .find(|(builtin, _)| builtin.eq_ignore_ascii_case(name))
        .map(|(builtin, bytes)| (*builtin, *bytes))
}

/// 3D 预览用的 wgpu 设备；和 Slint 渲染器共用同一个 instance / device / queue。
struct GpuContext {
    device: slint::wgpu_30::wgpu::Device,
    queue: slint::wgpu_30::wgpu::Queue,
}

/// 自己初始化 wgpu 并让 Slint 用它渲染；任何一步失败都返回 `None`，由调用方回退。
fn select_wgpu_backend() -> Option<GpuContext> {
    use slint::wgpu_30::wgpu;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .ok()?;

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("neutrino"),
        required_features: wgpu::Features::empty(),
        // 用 adapter 的真实上限：Slint 需要给整张壁纸（2559×1329）建纹理，
        // downlevel 的 2048 上限会直接 panic。
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .ok()?;

    slint::BackendSelector::new()
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Manual {
            instance,
            adapter,
            device: device.clone(),
            queue: queue.clone(),
        })
        .select()
        .ok()?;

    Some(GpuContext { device, queue })
}

/// 后台读取实例内容：扫描目录、解 zip、解码图标都不在 UI 线程做。
///
/// 每次点击分配一个 generation，回写模型前检查它是否还是最新的，
/// 这样快速切换实例时旧结果会被丢弃，不会盖掉新实例的列表。
#[derive(Default)]
struct InstanceContentLoader {
    generation: AtomicU64,
    /// 最近一次 `load` 的实例路径；内容改动完成后只在它还匹配时刷新，
    /// 避免后台操作覆盖用户刚切换到的另一个实例。
    current_path: Mutex<String>,
    cache: Mutex<HashMap<PathBuf, CachedArchive>>,
}

/// 已经解析过的 jar / zip 信息，按文件大小 + 修改时间判断是否还有效。
struct CachedArchive {
    len: u64,
    modified: Option<SystemTime>,
    version: String,
    loader: String,
    icon: Option<Arc<RgbaImage>>,
}

/// 后台线程准备好的行数据；`icon` 是已解码的 RGBA，UI 线程只做一次拷贝。
struct PreparedFile {
    name: String,
    size: u64,
    enabled: bool,
    version: String,
    loader: String,
    icon: Option<Arc<RgbaImage>>,
}

#[derive(Default)]
struct PreparedInstance {
    mods: Vec<PreparedFile>,
    resource_packs: Vec<PreparedFile>,
    shader_packs: Vec<PreparedFile>,
}

impl InstanceContentLoader {
    /// 最多缓存多少个文件的信息；超出后整体清空，避免长时间使用后无限增长。
    const CACHE_LIMIT: usize = 1024;

    fn load(self: &Arc<Self>, ui: &slint::Weak<AppWindow>, path: &str) {
        if let Ok(mut current) = self.current_path.lock() {
            current.clear();
            current.push_str(path);
        }

        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;

        if path.is_empty() {
            // 回到“全部实例”：直接清空列表，不能拿空路径去扫进程工作目录。
            self.publish_meta(ui, generation, instances::InstanceLoaderInfo::default());
            self.publish(ui, generation, PreparedInstance::default());
            return;
        }

        let this = self.clone();
        let weak = ui.clone();
        let path = path.to_string();

        std::thread::spawn(move || {
            let loader = instances::detect_instance_loader(&path);
            if this.is_stale(generation) {
                return;
            }
            this.publish_meta(&weak, generation, loader);

            let content = instances::detect_instance_content(&path);

            // 第一段：只带名称 / 大小 / 启用状态，先让列表立刻显示出来。
            if this.is_stale(generation) {
                return;
            }
            this.publish(&weak, generation, PreparedInstance::quick(&content));

            // 第二段：解开 jar 读版本 / 加载器 / 图标，结果到了再补一次。
            let full = PreparedInstance::full(&content, Path::new(&path), &this.cache);
            if !full.has_details() || this.is_stale(generation) {
                return;
            }
            this.publish(&weak, generation, full);
        });
    }

    fn publish_meta(
        self: &Arc<Self>,
        ui: &slint::Weak<AppWindow>,
        generation: u64,
        loader: instances::InstanceLoaderInfo,
    ) {
        let this = self.clone();
        let weak = ui.clone();

        let _ = weak.upgrade_in_event_loop(move |ui| {
            if this.is_stale(generation) {
                return;
            }

            let state = ui.global::<AppState>();
            state.set_instance_game_version(loader.game_version.into());
            state.set_instance_loader(loader.loader.into());
            state.set_instance_loader_name(loader.loader_name.into());
            state.set_instance_loader_version(loader.loader_version.into());
        });
    }

    fn is_stale(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) != generation
    }

    /// 内容改动完成后重新扫描；只有用户仍停在这个实例时才刷新列表。
    ///
    /// 必须在事件循环里调用：和用户切换实例的 `load` 串行执行，
    /// 否则「检查路径 → 重新扫描」之间可能插进一次切换。
    fn reload_if_current(self: &Arc<Self>, ui: &slint::Weak<AppWindow>, path: &str) {
        let is_current = self
            .current_path
            .lock()
            .map(|current| current.as_str() == path)
            .unwrap_or(false);

        if is_current {
            self.load(ui, path);
        }
    }

    fn publish(
        self: &Arc<Self>,
        ui: &slint::Weak<AppWindow>,
        generation: u64,
        prepared: PreparedInstance,
    ) {
        let this = self.clone();
        let weak = ui.clone();

        let _ = weak.upgrade_in_event_loop(move |ui| {
            if this.is_stale(generation) {
                return;
            }

            let state = ui.global::<AppState>();
            apply_files(state.get_mods(), prepared.mods, |model| {
                state.set_mods(model)
            });
            apply_files(
                state.get_resource_packs(),
                prepared.resource_packs,
                |model| state.set_resource_packs(model),
            );
            apply_files(state.get_shader_packs(), prepared.shader_packs, |model| {
                state.set_shader_packs(model)
            });
        });
    }
}

impl PreparedInstance {
    /// 扫目录就能拿到的部分（毫秒级）。
    fn quick(content: &instances::InstanceContent) -> Self {
        Self {
            mods: quick_files(&content.mods),
            resource_packs: quick_files(&content.resource_packs),
            shader_packs: quick_files(&content.shader_packs),
        }
    }

    /// 需要打开压缩包才能拿到的部分（版本号 / 加载器 / 图标）。
    fn full(
        content: &instances::InstanceContent,
        root: &Path,
        cache: &Mutex<HashMap<PathBuf, CachedArchive>>,
    ) -> Self {
        Self {
            mods: detailed_files(&content.mods, &root.join("mods"), cache),
            resource_packs: detailed_files(
                &content.resource_packs,
                &root.join("resourcepacks"),
                cache,
            ),
            shader_packs: detailed_files(&content.shader_packs, &root.join("shaderpacks"), cache),
        }
    }

    fn has_details(&self) -> bool {
        self.mods
            .iter()
            .chain(&self.resource_packs)
            .chain(&self.shader_packs)
            .any(|file| file.icon.is_some() || !file.version.is_empty() || !file.loader.is_empty())
    }
}

fn quick_files(files: &[instances::ContentFile]) -> Vec<PreparedFile> {
    files
        .iter()
        .map(|file| PreparedFile {
            name: file.name.clone(),
            size: file.size,
            enabled: file.enabled,
            version: String::new(),
            loader: String::new(),
            icon: None,
        })
        .collect()
}

fn detailed_files(
    files: &[instances::ContentFile],
    folder: &Path,
    cache: &Mutex<HashMap<PathBuf, CachedArchive>>,
) -> Vec<PreparedFile> {
    files
        .iter()
        .map(|file| {
            let storage_name = if file.enabled {
                file.name.clone()
            } else {
                format!("{}.disabled", file.name)
            };
            let (version, loader, icon) = read_archive_cached(&folder.join(storage_name), cache);

            PreparedFile {
                name: file.name.clone(),
                size: file.size,
                enabled: file.enabled,
                version,
                loader,
                icon,
            }
        })
        .collect()
}

/// 读 jar / zip 的附加信息，命中缓存时直接返回，切回同一个实例不会重新解压。
fn read_archive_cached(
    path: &Path,
    cache: &Mutex<HashMap<PathBuf, CachedArchive>>,
) -> (String, String, Option<Arc<RgbaImage>>) {
    let metadata = std::fs::metadata(path).ok();
    let len = metadata
        .as_ref()
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    let modified = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok());

    if let Ok(cache) = cache.lock()
        && let Some(hit) = cache.get(path)
        && hit.len == len
        && hit.modified == modified
    {
        return (hit.version.clone(), hit.loader.clone(), hit.icon.clone());
    }

    let info = instances::read_archive_info(path);
    let icon = info
        .icon
        .as_deref()
        .and_then(image_util::decode_rgba)
        .map(Arc::new);
    let entry = CachedArchive {
        len,
        modified,
        version: info.version,
        loader: info.loader,
        icon,
    };
    let result = (
        entry.version.clone(),
        entry.loader.clone(),
        entry.icon.clone(),
    );

    if let Ok(mut cache) = cache.lock() {
        if cache.len() >= InstanceContentLoader::CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(path.to_path_buf(), entry);
    }

    result
}

/// 行数没变时原地改每一行，避免第二次回填把列表滚动位置和悬停状态重置。
fn apply_files(
    existing: ModelRc<ContentFile>,
    files: Vec<PreparedFile>,
    replace: impl FnOnce(ModelRc<ContentFile>),
) {
    let rows = files
        .into_iter()
        .map(to_ui_content_file)
        .collect::<Vec<_>>();

    if let Some(model) = existing.as_any().downcast_ref::<VecModel<ContentFile>>()
        && model.row_count() == rows.len()
    {
        for (index, row) in rows.into_iter().enumerate() {
            model.set_row_data(index, row);
        }
        return;
    }

    replace(ModelRc::new(VecModel::from(rows)));
}

fn to_ui_content_file(file: PreparedFile) -> ContentFile {
    let icon = file.icon.map(|image| image_util::from_rgba(&image));
    let has_icon = icon.is_some();

    ContentFile {
        name: file.name.into(),
        size: i32::try_from(file.size).unwrap_or(i32::MAX),
        enabled: file.enabled,
        icon: icon.unwrap_or_default(),
        has_icon,
        version: file.version.into(),
        loader: file.loader.into(),
    }
}

fn lock_store(store: &Arc<Mutex<accounts::Store>>) -> std::sync::MutexGuard<'_, accounts::Store> {
    match store.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn avatar_for_skin(skin: &accounts::Skin) -> slint::Image {
    match skin {
        accounts::Skin::Steve => default_face(DEFAULT_STEVE_SKIN),
        accounts::Skin::Alex => default_face(DEFAULT_ALEX_SKIN),
        accounts::Skin::Custom(path) => skins::custom_avatar(path),
    }
}

fn default_face(bytes: &[u8]) -> slint::Image {
    image_util::decode_rgba(bytes)
        .map(|image| image_util::from_rgba(&image))
        .unwrap_or_default()
}

/// 读取 3D 预览需要的皮肤和披风字节；披风关闭时不读取文件。
fn preview_assets(store: &accounts::Store, name: &str) -> Option<(Vec<u8>, Option<Vec<u8>>, bool)> {
    let account = store
        .accounts()
        .iter()
        .find(|account| account.name == name)?;
    let skin = match &account.skin {
        accounts::Skin::Steve => DEFAULT_STEVE_SKIN.to_vec(),
        accounts::Skin::Alex => DEFAULT_ALEX_SKIN.to_vec(),
        accounts::Skin::Custom(path) => std::fs::read(path).ok()?,
    };
    let cape = if account.cape_enabled {
        account
            .cape
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
    } else {
        None
    };

    Some((skin, cape, account.cape_enabled))
}

fn update_account_state(ui: &AppWindow, store: &accounts::Store) {
    let accounts = store
        .accounts()
        .iter()
        .map(|account| {
            let (skin, avatar) = match &account.skin {
                accounts::Skin::Steve => (SkinType::Steve, slint::Image::default()),
                accounts::Skin::Alex => (SkinType::Alex, slint::Image::default()),
                accounts::Skin::Custom(path) => (SkinType::Custom, skins::custom_avatar(path)),
            };

            AccountInfo {
                name: account.name.clone().into(),
                kind: match account.kind {
                    accounts::AccountKind::Microsoft => AccountKind::Microsoft,
                    accounts::AccountKind::Offline => AccountKind::Offline,
                },
                skin,
                avatar,
                has_cape: account.cape.is_some(),
                cape_enabled: account.cape_enabled && account.cape.is_some(),
            }
        })
        .collect::<Vec<_>>();

    let state = ui.global::<AppState>();
    state.set_accounts(ModelRc::new(VecModel::from(accounts)));
    state.set_active_account(store.active().unwrap_or_default().into());
    state.set_active_account_avatar(
        store
            .active_account()
            .map(|account| avatar_for_skin(&account.skin))
            .unwrap_or_default(),
    );
    state.set_active_account_kind(
        match store.active_account().map(|account| account.kind) {
            Some(accounts::AccountKind::Microsoft) => "微软账户",
            Some(accounts::AccountKind::Offline) => "离线模式",
            None => "",
        }
        .into(),
    );
}

fn launch_game(
    store: &Arc<Mutex<accounts::Store>>,
    version: &str,
) -> Result<(String, std::process::Child), String> {
    let account = {
        let store = lock_store(store);
        store
            .active_account()
            .cloned()
            .ok_or_else(|| "请先在账户页选择一个角色".to_string())?
    };

    let account = match account.kind {
        accounts::AccountKind::Offline => core::launch::LaunchAccount::offline(&account.name),
        accounts::AccountKind::Microsoft => {
            let mut auth = account
                .microsoft
                .clone()
                .ok_or_else(|| "Microsoft 账号缺少认证信息，请重新登录".to_string())?;

            if token_needs_refresh(auth.expires_at) {
                let client_id = msa::client_id()?;
                let session = msa::refresh_session(client_id, auth.refresh_token.clone())?;
                auth.uuid = session.uuid.clone();
                auth.access_token = session.access_token.clone();
                auth.refresh_token = session.refresh_token.clone();
                auth.expires_at = session.expires_at;

                if let Err(error) = lock_store(store).update_microsoft_tokens(
                    &account.name,
                    &session.access_token,
                    &session.refresh_token,
                    session.expires_at,
                ) {
                    eprintln!("保存刷新后的 Microsoft token 失败：{error}");
                }
            }

            core::launch::LaunchAccount {
                name: account.name.clone(),
                uuid: auth.uuid,
                access_token: auth.access_token,
                user_type: "msa".to_string(),
            }
        }
    };

    let java = java::detect_java().ok_or_else(|| "未检测到 Java 运行时".to_string())?;
    let executable = java.executable();
    if !executable.is_file() {
        return Err(format!("找不到 java 可执行文件：{}", executable.display()));
    }

    if let Some(required) = required_java_major(version)
        && java.major < required as i32
    {
        eprintln!(
            "警告：{version} 需要 Java {required}，当前检测到 Java {}",
            java.major
        );
    }

    let options = core::launch::LaunchOptions::new(
        core::default_game_dir(),
        version.to_string(),
        account,
        executable,
    );

    let child = core::launch::spawn(&options).map_err(|error| error.to_string())?;
    Ok((format!("已启动 {version}（PID {}）", child.id()), child))
}

fn token_needs_refresh(expires_at: u64) -> bool {
    expires_at <= unix_now().saturating_add(60)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn required_java_major(version: &str) -> Option<u32> {
    let path = core::GamePaths::new(core::default_game_dir()).version_json(version);
    let bytes = std::fs::read(path).ok()?;
    let json: core::VersionJson = serde_json::from_slice(&bytes).ok()?;
    json.java_version
        .map(|java| java.major_version)
        .filter(|major| *major > 0)
}

fn open_folder(path: &str) -> std::io::Result<()> {
    if path.is_empty() {
        return Ok(());
    }

    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };

    Command::new(opener).arg(path).spawn()?;
    Ok(())
}

pub(crate) fn open_url(url: &str) -> std::io::Result<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unsupported URL scheme",
        ));
    }

    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };

    command.arg(url).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_refresh_window() {
        assert!(token_needs_refresh(0));
        assert!(!token_needs_refresh(unix_now() + 3600));
    }
}
