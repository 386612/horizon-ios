//! The startup's resource load: the stages the loading screen shows, the
//! scoped threads each step runs on, and the core, gameplay and pack tables
//! read from the prepared resources.
use super::*;

pub(super) const STAGE_LINES: [&str; 10] = [
    "Loading menu",
    "Loading assets",
    "Loading block facts",
    "Loading biomes",
    "Loading pack tables",
    "Loading collision",
    "Loading items",
    "Loading recipes",
    "Loading world resources",
    "World resources ready",
];

/// The background loader's checkpoint, read by the loading screen each frame.
#[derive(Clone)]
pub(super) struct Progress {
    pub(super) value: Arc<AtomicU32>,
    pub(super) lines: &'static [&'static str],
}
impl Progress {
    pub(super) fn new() -> Self {
        Self::with(&STAGE_LINES)
    }
    pub(super) fn with(lines: &'static [&'static str]) -> Self {
        Self {
            value: Arc::new(AtomicU32::new(0)),
            lines,
        }
    }
    pub(super) fn set(&self, stage: u32) {
        self.value.store(stage, Ordering::Relaxed);
    }
    pub(super) fn fraction(&self) -> f32 {
        self.value.load(Ordering::Relaxed) as f32 / (self.lines.len() - 1) as f32
    }
    pub(super) fn line(&self) -> &'static str {
        self.lines[(self.value.load(Ordering::Relaxed) as usize).min(self.lines.len() - 1)]
    }
}

/// Harness and dedicated runs keep the fully-synchronous startup so their
/// output stays byte-identical; only an ordinary launch shows the window first.
pub(super) fn synchronous_startup(options: &Options) -> bool {
    // A headless game has no window for the loading screen.
    options.headless
        || options.frames.is_some()
        || options.world_frames.is_some()
        || options.benchmark.is_some()
        // Native launcher automation must observe the same idle/start lifecycle as touch input.
        || (options.control.is_some()
            && !horizon_ios_launcher::waiting()
            && !import_loading_screen(options))
        || !options.execution_role.is_client()
        || std::env::var_os("HORIZON_DISPENSE_FIXTURE").is_some()
        || std::env::var_os("HORIZON_SCULK_FIXTURE").is_some()
        || std::env::var_os("HORIZON_COMMAND_SCRIPT").is_some()
        || std::env::var_os("HORIZON_NET_REPORT").is_some()
}

/// The local world a client's launch opens at once (`--world`,
/// `--quickPlaySingleplayer`), whose generator data can load with the game's.
pub(super) fn launch_world(options: &Options) -> Option<&str> {
    (options.execution_role.is_client() && options.server.is_none() && !options.test_terrain)
        .then_some(options.world.as_deref())
        .flatten()
}

/// A scoped thread with a name, for one step of the resource load.
///
/// Named because the heap census and the startup trace both fall back to the
/// OS thread name: unnamed, every loader's retained memory and every span it
/// ran landed in one `unnamed thread` bucket. That bucket was 330 MiB of a
/// playing game's resident set -- the largest of all of them -- with nothing
/// to say which loader held it, and 13 of the trace's 17 lanes were
/// indistinguishable.
pub(super) fn named_scope<'scope, T: Send + 'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    name: &str,
    f: impl FnOnce() -> T + Send + 'scope,
) -> std::thread::ScopedJoinHandle<'scope, T> {
    std::thread::Builder::new()
        // Linux keeps 15 bytes of a thread name: a short prefix keeps them apart
        // ("horizon-load-core" and "horizon-load-collision" would both read
        // "horizon-load-co").
        .name(format!("load-{name}"))
        .spawn_scoped(scope, f)
        // `Scope::spawn`, which this replaces, panics when the OS refuses a
        // thread. Keeping that: a resource load cannot go on without it.
        .expect("resource loader thread")
}

/// A step of the resource load: on its own scoped thread, or, when the
/// settings allow a single worker, run where it is joined.
pub(super) enum Step<'scope, T> {
    Thread(std::thread::ScopedJoinHandle<'scope, T>),
    Later(Box<dyn FnOnce() -> T + Send + 'scope>),
}
impl<'scope, T: Send + 'scope> Step<'scope, T> {
    /// `name` names the thread when one is spawned ([`named_scope`]).
    pub(super) fn spawn(
        scope: &'scope std::thread::Scope<'scope, '_>,
        parallel: bool,
        name: &str,
        f: impl FnOnce() -> T + Send + 'scope,
    ) -> Self {
        if parallel {
            Self::Thread(named_scope(scope, name, f))
        } else {
            Self::Later(Box::new(f))
        }
    }
    pub(super) fn join(self) -> T {
        match self {
            Self::Thread(handle) => handle.join().unwrap(),
            Self::Later(f) => f(),
        }
    }
}

/// The world bundle's core tables from one resource root, before
/// `load_resources` binds them into the pack tables and `WorldCore`.
pub(super) struct CoreTables {
    pub(super) assets: Assets,
    pub(super) entity_models: Option<Arc<horizon_client::mobs::EntityModelDocument>>,
    pub(super) item_defaults: Arc<horizon_world::item_defaults::Defaults>,
    pub(super) render: RenderAssets,
    pub(super) item_renderer: horizon_client::item_render::ItemRenderer,
    pub(super) visibility: horizon_world::block_render::FaceVisibility,
    pub(super) gameplay: GameplayTables,
    pub(super) biomes: Arc<Biomes>,
    pub(super) environment: Environment,
    pub(super) collision: Arc<BlockCollisionRegistry<'static>>,
    pub(super) ysm_model: Option<Arc<LoadedPlayerYsm>>,
}
/// A player YSM decoded before the renderer builds its sprite atlas. Its
/// extracted textures must exist before RenderAssets::load enumerates atlas
/// sprites; loading the file later in load_world_rest is too late for the atlas.
pub(super) struct LoadedPlayerYsm {
    pub(super) path: std::path::PathBuf,
    pub(super) loaded: horizon_ysm::LoadedYsm,
    pub(super) main_part: horizon_client::mobs::Part,
    pub(super) texture_path: String,
}

fn ysm_safe_component(value: &str) -> String {
    let value: String = value.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .take(64)
        .collect();
    if value.is_empty() { "model".to_owned() } else { value }
}

pub(super) fn ysm_texture_resource_path(path: &std::path::Path, texture_name: &str) -> String {
    let stem = path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("model");
    format!(
        "entity/player/ysm/{}/{}",
        ysm_safe_component(stem),
        ysm_safe_component(texture_name),
    )
}

fn extract_ysm_textures(
    game_root: &std::path::Path,
    path: &std::path::Path,
    loaded: &horizon_ysm::LoadedYsm,
) -> std::io::Result<()> {
    let stem = path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("model");
    let texture_root = game_root
        .join("config/ysm/models/.textures")
        .join(ysm_safe_component(stem));
    if texture_root.exists() {
        std::fs::remove_dir_all(&texture_root)?;
    }
    std::fs::create_dir_all(&texture_root)?;
    for texture in &loaded.resource.textures {
        let name = ysm_safe_component(&texture.name);
        std::fs::write(texture_root.join(format!("{name}.png")), &texture.png_bytes)?;
    }
    Ok(())
}

/// Select and decode the first usable model once, before atlas construction.
/// The same parsed result is passed to load_world_rest, avoiding a second
/// Crypto v3 decrypt/parse pass during startup.
fn load_player_ysm(game_root: &std::path::Path) -> Option<LoadedPlayerYsm> {
    let mut paths = match horizon_ysm::discover_models(game_root) {
        Ok(paths) => paths,
        Err(error) => {
            log::warn!("Could not scan config/ysm/models: {error}");
            return None;
        }
    };
    if let Some(selected) = std::fs::read_to_string(game_root.join("config/ysm/active.txt"))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        if let Some(index) = paths.iter().position(|path| {
            path.file_name().is_some_and(|name| name.to_string_lossy() == selected)
        }) {
            paths.swap(0, index);
        }
    }
    for path in paths {
        let loaded = match horizon_ysm::load_ysm_file(&path) {
            Ok(loaded) => loaded,
            Err(error) => {
                log::warn!("YSM model {} could not be loaded: {error}", path.display());
                continue;
            }
        };
        let Some((_, model)) = loaded.resource.models.iter().find(|(name, _)| name == "main") else {
            log::warn!("YSM model {} has no main model", path.display());
            continue;
        };
        let main_part = match horizon_client::mobs::ysm_model::convert_ysm_model(model) {
            Ok(part) => part,
            Err(error) => {
                log::warn!("YSM model {} could not be converted: {error}", path.display());
                continue;
            }
        };
        let default_texture = loaded.resource.metadata.as_ref()
            .map(|metadata| metadata.default_texture.as_str())
            .filter(|name| !name.is_empty());
        let Some(texture) = default_texture
            .and_then(|name| loaded.resource.textures.iter().find(|texture| texture.name == name))
            .or_else(|| loaded.resource.textures.iter().find(|texture| {
                !texture.name.ends_with("_normal")
                    && !texture.name.ends_with("_specular")
                    && !texture.name.ends_with("_special")
            }))
        else {
            log::warn!("YSM model {} has no base texture", path.display());
            continue;
        };
        if let Err(error) = extract_ysm_textures(game_root, &path, &loaded) {
            log::warn!("YSM textures from {} could not be extracted: {error}", path.display());
            continue;
        }
        let texture_path = ysm_texture_resource_path(&path, &texture.name);
        return Some(LoadedPlayerYsm {
            path,
            loaded,
            main_part,
            texture_path,
        });
    }
    None
}

/// The block facts and the tables read from them.
pub(super) struct GameplayTables {
    pub(super) behaviour_json: serde_json::Value,
    pub(super) facts: BlockFacts,
    pub(super) gameplay_projection: Vec<horizon_client::blocks::GameplayProjection>,
    pub(super) light: LightTables,
    pub(super) motion: MotionResources,
}

/// [`CoreTables`] at `root`. Every table reads the block registry; past it,
/// the block models and sprites (then the item models, which share their
/// parsed models), the face visibility, the collisions, the biomes and the
/// environment need no block facts and load beside them. The outer error
/// fails the load; the inner one is the packs' (the registry or the block
/// models), which `load_resources` rolls back from.
pub(super) fn load_core(
    root: &std::path::Path,
    headless: bool,
    parallel: bool,
    progress: &Progress,
    launch_world: Option<&str>,
) -> std::result::Result<std::result::Result<CoreTables, String>, String> {
    // YSM textures are ordinary player-atlas sprites. Extract them before
    // Assets::reload enumerates atlas sources, then reuse this parsed model
    // during the later mob-table load instead of decrypting it a second time.
    let ysm_model = if headless {
        None
    } else {
        let game_root = std::env::current_dir().unwrap_or_default();
        startup::time("core.ysm_player_model", || load_player_ysm(&game_root)).map(Arc::new)
    };
    // The model library reads every file under `assets/` and needs no
    // block registry, so it is parsed beside `Assets::load` rather than
    // after it. It used to sit inside `reload` behind both the registry
    // read and the block facts; on a cold cache that read was most of a
    // second on the critical path, and warm it is still ~30 ms.
    // A server draws nothing and parses no models, so it skips this.
    let (assets, parsed_models, entity_models) = std::thread::scope(|scope| {
        let models = (parallel && !headless).then(|| {
            scope.spawn(|| {
                let _step = startup::step("core.models_parse");
                horizon_client::model_library::ParsedModels::load(root)
            })
        });
        let entity_models = (!headless).then(|| {
            Step::spawn(scope, parallel, "entity_models", || {
                startup::time("core.entity_models", || {
                    horizon_client::mobs::EntityModelDocument::load(root).ok()
                })
            })
        });
        let assets = startup::time("core.assets", || Assets::load(root));
        let models = models.map(|m| m.join().unwrap_or_else(|e| std::panic::resume_unwind(e)));
        let entity_models = entity_models.and_then(|step| step.join()).map(Arc::new);
        (assets, models, entity_models)
    });
    let assets = match assets {
        Ok(assets) => assets,
        Err(e) => return Ok(Err(e.to_string())),
    };
    progress.set(1);
    let registry = assets.registry;
    let entity_models_for_render = entity_models.clone();
    let (render, gameplay, visibility, biomes, environment, collision) = std::thread::scope(
        |scope| {
            let assets = &assets;
            let render = Step::spawn(scope, parallel, "render", move || {
                // A server reads no block sprites' pixels, which only the
                // renderer draws (32 MB).
                let parsed_models = parsed_models;
                let render = startup::time("core.block_models_sprites", || {
                    if headless {
                        assets.reload_for_server(root)
                    } else if let Some(parsed) = parsed_models {
                        assets.reload_with_models(root, parsed)
                    } else {
                        assets.reload(root)
                    }
                })
                .map_err(|e| e.to_string())?;
                let library = render.model_library.clone();
                let item_defaults = startup::time("core.item_defaults", || {
                    horizon_world::item_defaults::shared(root)
                })
                .map_err(|e| e.to_string())?;
                let item_renderer = startup::time("core.item_models", || {
                    if headless {
                        horizon_client::item_render::ItemRenderer::load_for_server(root, library)
                    } else if let Some(document) = entity_models_for_render.as_deref() {
                        horizon_client::item_render::ItemRenderer::load_with_library_and_entity_models(
                            root, library, document,
                        )
                    } else {
                        horizon_client::item_render::ItemRenderer::load_with_library(root, library)
                    }
                })
                .map_err(|e| e.to_string());
                Ok((render, item_renderer, item_defaults))
            });
            // Nor the face visibility the mesher and the block entity
            // renderer cull with (19 MB).
            let visibility = Step::spawn(scope, parallel, "visibility", || {
                if headless {
                    return Ok(horizon_world::block_render::FaceVisibility::empty());
                }
                startup::time("core.visibility", || {
                    models::load_visibility(root, registry)
                })
                .map_err(|e| e.to_string())
            });
            let biomes = Step::spawn(scope, parallel, "biomes", || {
                let biomes = Arc::new(
                    startup::time("core.biomes", || Biomes::load(root))
                        .map_err(|e| e.to_string())?,
                );
                // A launch that names a world: its data preload needs only the
                // block registry and the biomes, so it starts here rather than
                // after the block models and facts (~250 ms later).
                if let Some(folder) = launch_world {
                    world_loading::preload_launch_world(folder, root, registry, biomes.clone());
                }
                Ok::<_, String>(biomes)
            });
            let environment = Step::spawn(scope, parallel, "environment", || {
                startup::time("core.environment", || Environment::load(root))
                    .map_err(|e| e.to_string())
            });
            let collision = Step::spawn(scope, parallel, "collision", || {
                startup::time("core.collision", || collisions::load(root, registry))
                    .map_err(|e| e.to_string())
            });
            let gameplay = gameplay_tables(root, assets, parallel, progress);
            (
                render.join(),
                gameplay,
                visibility.join(),
                biomes.join(),
                environment.join(),
                collision.join(),
            )
        },
    );
    // The packs' failure first: it rolls back whatever else failed.
    let (render, item_renderer, item_defaults) = match render {
        Ok(render) => render,
        Err(e) => return Ok(Err(e)),
    };
    let gameplay = gameplay?;
    progress.set(3);
    Ok(Ok(CoreTables {
        item_defaults,
        entity_models,
        gameplay,
        biomes: biomes?,
        environment: environment?,
        visibility: visibility?,
        item_renderer: item_renderer?,
        collision: Arc::new(collision?),
        ysm_model,
        assets,
        render,
    }))
}
/// The block facts, then the light tables beside the motion tables, which
/// both read them.
pub(super) fn gameplay_tables(
    root: &std::path::Path,
    assets: &Assets,
    parallel: bool,
    progress: &Progress,
) -> std::result::Result<GameplayTables, String> {
    let facts_step = startup::step("core.block_facts");
    // Read whole, then parsed from the buffer: `serde_json::from_reader`
    // pulls through a `BufReader` and is measurably slower on a file this
    // size than one read plus `from_slice`, which is how `BlockFacts` next
    // door already reads its own. Same value either way.
    let behaviour_json: serde_json::Value =
        horizon_world::json_file::read(&root.join("block-behaviours.json"))
            .map_err(|e| e.to_string())?;
    let (facts, gameplay_projection) =
        BlockFacts::load_with_projection(root, assets.registry, &behaviour_json, parallel)
            .map_err(|e| e.to_string())?;
    horizon_client::block_contact::set_mod_blocks(
        facts
            .states
            .iter()
            .any(|s| !s.name.starts_with("minecraft:")),
    );
    drop(facts_step);
    progress.set(2);
    let (light, motion) = std::thread::scope(|scope| {
        let light = Step::spawn(scope, parallel, "light", || {
            startup::time("core.light", || {
                LightTables::load(root, assets.registry, &facts)
            })
            .map_err(|e| e.to_string())
        });
        let motion = startup::time("core.motion", || {
            MotionResources::load_with_facts(root, assets, &facts)
        })
        .map_err(|e| e.to_string());
        (light.join(), motion)
    });
    Ok(GameplayTables {
        light: light?,
        motion: motion?,
        behaviour_json,
        facts,
        gameplay_projection,
    })
}

/// The synchronous launch loads both phases; an ordinary launch returns the
/// core bundle while the rest worker keeps loading. `launch_world`: the world
/// the launch opens, whose data is preloaded meanwhile.
#[allow(clippy::too_many_arguments)]
pub(super) fn load_resources(
    root: PathBuf,
    launch_world: Option<&str>,
    vanilla_root: &std::path::Path,
    settings: &Settings,
    started: Instant,
    menu: bool,
    headless: bool,
    progress: &Progress,
    publish: &mut dyn FnMut(UiResources, &std::path::Path),
) -> Result<Loaded> {
    load_resources_with_access(
        root,
        launch_world,
        vanilla_root,
        settings,
        started,
        menu,
        headless,
        progress,
        None,
        publish,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn load_resources_with_access(
    mut root: PathBuf,
    launch_world: Option<&str>,
    vanilla_root: &std::path::Path,
    settings: &Settings,
    started: Instant,
    menu: bool,
    headless: bool,
    progress: &Progress,
    home_access: Option<horizon_client::import::HomeAccess>,
    publish: &mut dyn FnMut(UiResources, &std::path::Path),
) -> Result<Loaded> {
    let threads = settings.horizon.worker_threads();
    // One pool of workers for every level's streaming, the client's and the
    // server thread's (horizon_threads::jobs), sized before any world opens.
    horizon_threads::jobs::configure(threads);
    horizon_client::sprites::set_texture_budget(settings.horizon.texture_budget_mib);
    horizon_client::sprites::set_labpbr_maps(shader_packs::picks_material_pack(settings));
    // Stage A: the menu bundle (atlas and panorama). With a loading screen it is
    // published at once so the title screen can appear while the world bundle
    // loads; the synchronous launch instead carries it in `Loaded`.
    let mut pack_error = None;
    // A dedicated server builds only the font (UiResources::server), as the
    // integrated server's game does: the full atlas and panorama are 144 MB.
    let load_ui = |root: &std::path::Path| {
        let _step = startup::step("ui.load");
        if headless {
            UiResources::server(root)
        } else {
            UiResources::load(root, vanilla_root)
        }
    };
    // The menu bundle loads on this thread while workers build the world
    // bundle's core tables from the same root (`load_core`). A pack failure
    // in either starts both again with the game's own resources
    // (Minecraft.rollbackResourcePacks).
    let parallel = threads > 1;
    let mut offer = |ui: UiResources, root: &std::path::Path| {
        if menu {
            log::info!(
                "Horizon: UI bundle ready after {} ms",
                started.elapsed().as_millis()
            );
            publish(ui, root);
            None
        } else {
            Some(ui)
        }
    };
    let (core, loaded_gui) = loop {
        let (ui, core) = std::thread::scope(|scope| {
            let core = Step::spawn(scope, parallel, "core", || {
                // A failed pack starts core again from the game's own
                // resources, which the world's preload must read: with packs
                // it waits for the core that loaded.
                let early = launch_world.filter(|_| root == vanilla_root);
                startup::time("core", || {
                    load_core(&root, headless, parallel, progress, early)
                })
            });
            let ui = load_ui(&root).map(|ui| offer(ui, &root));
            (ui, core.join())
        });
        let packs = root != vanilla_root;
        let failure = match (ui, core) {
            (Ok(gui), Ok(Ok(core))) => break (core, gui),
            (Err(e), _) if packs => e.to_string(),
            (_, Ok(Err(e))) if packs => e,
            (_, Ok(Err(e))) => {
                return Err(format!(
                    "{e}\nPrepare resources with the launcher, or: horizon-prepare --prepare"
                )
                .into());
            }
            (_, Err(e)) => return Err(e.into()),
            (Err(e), Ok(Ok(_))) => return Err(e),
        };
        log::warn!(
            "Horizon: resource packs failed to load ({failure}); using the default resources"
        );
        pack_error = Some(failure);
        root = vanilla_root.to_path_buf();
    };
    log::info!("load core tables: {:.2}s", started.elapsed().as_secs_f32());
    let CoreTables {
        assets,
        entity_models,
        item_defaults,
        render,
        item_renderer,
        visibility,
        gameplay:
            GameplayTables {
                behaviour_json,
                facts,
                gameplay_projection,
                light,
                motion,
            },
        biomes,
        environment,
        collision,
        ysm_model,
    } = core;
    let assets = Arc::new(assets);
    let render = Arc::new(render);
    if let Some(folder) = launch_world {
        world_loading::preload_launch_world(folder, &root, assets.registry, biomes.clone());
    }
    let tables = Arc::new(startup::time("pack.mesh_tables", || {
        MeshTables::new(&assets, &render, &facts, visibility)
            .map(|t| t.with_polytone_colormaps(&root, assets.registry))
    })?);
    let pack_step = startup::step("pack.assemble");
    let pack = PackResources::assemble(
        &root,
        assets.clone(),
        render,
        tables,
        item_renderer,
        biomes.clone(),
        Arc::new(light),
    );
    drop(pack_step);
    log::info!("load pack tables: {:.2}s", started.elapsed().as_secs_f32());
    progress.set(4);
    if !menu {
        horizon_client::ui::load_metadata(&root);
        horizon_client::lang::set(horizon_client::lang::Language::load(
            &[&root],
            &settings.language,
        ));
    }
    progress.set(5);
    let suffocation =
        horizon_client::mobs::suffocation::bind(collision.clone(), &motion.causes_suffocation)?;
    let resources = Resources {
        core: WorldCore {
            facts,
            biomes: biomes.clone(),
            environment,
            collision,
            motion: Arc::new(motion),
            suffocation,
            threads,
        },
        rest: std::sync::OnceLock::new(),
    };
    let resources = Arc::new(resources);
    if menu {
        horizon_client::telemetry::mark_core_ready();
        log::info!(
            "Horizon: core-ready after {} ms",
            started.elapsed().as_millis()
        );
    }
    // The block entity renderer's face visibility: the meshes' own, whose
    // shapes already hold their faces, not a second load of the same tables.
    let visibility = pack.context.tables.visibility.clone();
    let rest_loader = if menu {
        let home_access = home_access.clone();
        let resources = resources.clone();
        let root = root.clone();
        let root_lease = horizon_assets::root_lease::lease_for(&root);
        let vanilla_root = vanilla_root.to_owned();
        let entity_models = entity_models.clone();
        let ysm_model = ysm_model.clone();
        let rest_progress = progress.clone();
        let progress = progress.clone();
        let handle = std::thread::Builder::new()
            .name("world-rest-loader".into())
            .spawn(move || {
                let _root_lease = root_lease;
                let _step = startup::step("rest");
                let rest = load_world_rest(
                    &root,
                    &resources,
                    &assets,
                    &behaviour_json,
                    &gameplay_projection,
                    entity_models.as_deref(),
                    ysm_model.as_deref(),
                    item_defaults.as_ref(),
                    visibility,
                    &progress,
                    started,
                    true,
                )
                .map_err(|error| error.to_string())?;
                resources
                    .rest
                    .set(rest)
                    .map_err(|_| "world resources already loaded".to_owned())?;
                progress.set(9);
                horizon_client::telemetry::mark_rest_ready();
                log::info!(
                    "Horizon: rest-ready after {} ms",
                    started.elapsed().as_millis()
                );
                world_loading::write_prepared_files(vanilla_root, home_access);
                Ok(())
            })?;
        Some(WorldRestLoader {
            progress: rest_progress,
            handle: Some(handle),
        })
    } else {
        let rest = load_world_rest(
            &root,
            &resources,
            &assets,
            &behaviour_json,
            &gameplay_projection,
            entity_models.as_deref(),
            ysm_model.as_deref(),
            item_defaults.as_ref(),
            visibility,
            progress,
            started,
            !headless || std::env::var_os("HORIZON_AVATAR_EQUIPMENT_FIXTURE").is_some(),
        )?;
        resources
            .rest
            .set(rest)
            .map_err(|_| "world resources already loaded")?;
        None
    };
    // The world tables load (above) while the handoff's CPU work is done: the
    // world renderer's textures beside the game's own tables. A failure here
    // waits for the tail, so no tail worker outlives its pack-loader generation.
    let prepared_at = Instant::now();
    let (game_cpu, renderer_cpu) = if menu {
        let (game_cpu, renderer_cpu) = std::thread::scope(|scope| {
            let textures = Step::spawn(scope, parallel, "textures", || {
                startup::time("handoff.renderer_textures", || {
                    prepare_renderer_textures(&pack)
                })
            });
            let game_cpu = startup::time("handoff.game_cpu", || {
                GameCpuResources::load(&root, entity_models.as_deref())
            });
            (game_cpu, textures.join())
        });
        let game_cpu = match game_cpu {
            Ok(game_cpu) => game_cpu,
            Err(error) => {
                if let Some(tail) = rest_loader {
                    let _ = tail.finish();
                }
                return Err(error);
            }
        };
        log::info!(
            "Horizon: handoff CPU resources prepared on worker in {} ms",
            prepared_at.elapsed().as_millis()
        );
        (Some(game_cpu), Some(renderer_cpu))
    } else {
        (None, None)
    };
    Ok(Loaded {
        home_access,
        game_cpu,
        entity_models,
        renderer_cpu,
        root,
        pack_error,
        resources,
        rest_loader,
        pack,
        gui: loaded_gui,
    })
}


#[cfg(test)]
mod ysm_texture_path_tests {
    use super::*;

    #[test]
    fn ysm_texture_resource_and_extraction_paths_share_the_same_components() {
        let model = std::path::Path::new("1.ysm");
        assert_eq!(
            ysm_texture_resource_path(model, "aru"),
            "entity/player/ysm/1/aru",
        );
        assert_eq!(
            ysm_texture_resource_path(std::path::Path::new("A model.ysm"), "body/diffuse"),
            "entity/player/ysm/Amodel/bodydiffuse",
        );
    }
}
