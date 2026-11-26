//! Tile production on worker threads and the GPU tile cache.

use crate::synth::{TileKey, TILE};
use crate::TileSource;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

#[derive(Default)]
struct Requests {
    /// Tiles the current view wants, most urgent first.
    wanted: Vec<TileKey>,
    /// Tiles a worker is computing or that wait to be uploaded.
    claimed: HashSet<TileKey>,
    stop: bool,
}

/// Computes tiles off the UI thread. The view replaces the wanted list every
/// frame, so a tile the view has left is never started.
pub struct TileProducer {
    requests: Arc<(Mutex<Requests>, Condvar)>,
    finished: Receiver<(TileKey, Vec<u16>)>,
    workers: Vec<std::thread::JoinHandle<()>>,
    /// Microseconds spent computing tiles, and how many were computed.
    spent: Arc<(AtomicU64, AtomicU64)>,
}

impl TileProducer {
    pub fn new(worker_count: usize, source: Arc<dyn TileSource>) -> Self {
        let requests = Arc::new((Mutex::new(Requests::default()), Condvar::new()));
        let (sender, finished) = channel();
        let spent = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
        let workers = (0..worker_count)
            .map(|index| {
                let requests = requests.clone();
                let sender: Sender<(TileKey, Vec<u16>)> = sender.clone();
                let spent = spent.clone();
                let source = source.clone();
                std::thread::Builder::new()
                    .name(format!("tile-{index}"))
                    .spawn(move || worker(&requests, &sender, &spent, source.as_ref()))
                    .expect("spawning a tile worker")
            })
            .collect();
        Self {
            requests,
            finished,
            workers,
            spent,
        }
    }

    pub fn set_wanted(&self, wanted: Vec<TileKey>) {
        let (lock, wake) = &*self.requests;
        lock.lock().expect("tile requests").wanted = wanted;
        wake.notify_all();
    }

    /// Takes one finished tile. The caller uploads it and then calls
    /// [`Self::uploaded`].
    pub fn take_finished(&self) -> Option<(TileKey, Vec<u16>)> {
        self.finished.try_recv().ok()
    }

    pub fn uploaded(&self, key: TileKey) {
        self.requests
            .0
            .lock()
            .expect("tile requests")
            .claimed
            .remove(&key);
    }

    pub fn mean_tile_ms(&self) -> f32 {
        let micros = self.spent.0.load(Ordering::Relaxed) as f32;
        let count = self.spent.1.load(Ordering::Relaxed).max(1) as f32;
        micros / count / 1000.0
    }

    pub fn in_flight(&self) -> usize {
        self.requests.0.lock().expect("tile requests").claimed.len()
    }
}

impl Drop for TileProducer {
    fn drop(&mut self) {
        let (lock, wake) = &*self.requests;
        lock.lock().expect("tile requests").stop = true;
        wake.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn worker(
    requests: &(Mutex<Requests>, Condvar),
    sender: &Sender<(TileKey, Vec<u16>)>,
    spent: &(AtomicU64, AtomicU64),
    source: &dyn TileSource,
) {
    let (lock, wake) = requests;
    loop {
        let key = {
            let mut guard = lock.lock().expect("tile requests");
            loop {
                if guard.stop {
                    return;
                }
                let next = guard
                    .wanted
                    .iter()
                    .copied()
                    .find(|key| !guard.claimed.contains(key));
                if let Some(key) = next {
                    guard.claimed.insert(key);
                    break key;
                }
                guard = wake.wait(guard).expect("tile requests");
            }
        };
        let started = Instant::now();
        let samples = source.tile(key);
        spent
            .0
            .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        spent.1.fetch_add(1, Ordering::Relaxed);
        if sender.send((key, samples)).is_err() {
            return;
        }
    }
}

pub struct GpuTile {
    pub bind_group: wgpu::BindGroup,
    _texture: wgpu::Texture,
    last_used: u64,
}

/// Tiles held on the GPU, within a byte budget. The tile least recently drawn
/// is dropped first. Pinned tiles are never dropped.
pub struct TileCache {
    tiles: HashMap<TileKey, GpuTile>,
    pinned: HashSet<TileKey>,
    budget_bytes: u64,
    /// The layers in every tile.
    bands: usize,
}

impl TileCache {
    pub fn new(budget_bytes: u64, bands: usize) -> Self {
        Self {
            tiles: HashMap::new(),
            pinned: HashSet::new(),
            budget_bytes,
            bands,
        }
    }

    /// The bytes one tile takes on the GPU.
    pub fn tile_bytes(&self) -> u64 {
        TILE as u64 * TILE as u64 * self.bands as u64 * 2
    }

    pub fn contains(&self, key: TileKey) -> bool {
        self.tiles.contains_key(&key)
    }

    pub fn bytes(&self) -> u64 {
        self.tiles.len() as u64 * self.tile_bytes()
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn pin(&mut self, key: TileKey) {
        self.pinned.insert(key);
    }

    pub fn get(&self, key: TileKey) -> Option<&GpuTile> {
        self.tiles.get(&key)
    }

    /// Marks a tile as drawn this frame and returns it.
    pub fn touch(&mut self, key: TileKey, frame: u64) -> Option<&GpuTile> {
        let tile = self.tiles.get_mut(&key)?;
        tile.last_used = frame;
        Some(tile)
    }

    pub fn insert(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        key: TileKey,
        samples: &[u16],
        frame: u64,
    ) {
        let size = wgpu::Extent3d {
            width: TILE,
            height: TILE,
            depth_or_array_layers: self.bands as u32,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tile"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(samples),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(TILE * 2),
                rows_per_image: Some(TILE),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tile"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        self.tiles.insert(
            key,
            GpuTile {
                bind_group,
                _texture: texture,
                last_used: frame,
            },
        );
    }

    /// Drops the least recently drawn tiles until the cache fits its budget.
    /// A tile drawn in `frame` is kept even when the cache is over budget.
    pub fn evict(&mut self, frame: u64) -> usize {
        let over = self.bytes().saturating_sub(self.budget_bytes);
        if over == 0 {
            return 0;
        }
        let mut candidates: Vec<(u64, TileKey)> = self
            .tiles
            .iter()
            .filter(|(key, tile)| tile.last_used < frame && !self.pinned.contains(key))
            .map(|(key, tile)| (tile.last_used, *key))
            .collect();
        candidates.sort_unstable();
        let count = (over.div_ceil(self.tile_bytes()) as usize).min(candidates.len());
        for (_, key) in &candidates[..count] {
            self.tiles.remove(key);
        }
        count
    }
}
