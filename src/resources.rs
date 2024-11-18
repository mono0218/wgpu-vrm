use std::io::{BufReader, Cursor};

use cfg_if::cfg_if;
use wgpu::util::DeviceExt;

use crate::{model, texture};
use gltf::Gltf;
use crate::model::ModelVertex;

#[cfg(target_arch = "wasm32")]
fn format_url(file_name: &str) -> reqwest::Url {
    let window = web_sys::window().unwrap();
    let location = window.location();
    let mut origin = location.origin().unwrap();
    if !origin.ends_with("learn-wgpu") {
        origin = format!("{}/learn-wgpu", origin);
    }
    let base = reqwest::Url::parse(&format!("{}/", origin,)).unwrap();
    base.join(file_name).unwrap()
}

pub async fn load_string(file_name: &str) -> anyhow::Result<String> {
    cfg_if! {
        if #[cfg(target_arch = "wasm32")] {
            let url = format_url(file_name);
            let txt = reqwest::get(url)
                .await?
                .text()
                .await?;
        } else {
            let path = std::path::Path::new(env!("OUT_DIR"))
                .join("res")
                .join(file_name);
            let txt = path.to_str().unwrap().to_string();
        }
    }

    Ok(txt)
}

pub async fn load_binary(file_name: &str) -> anyhow::Result<Vec<u8>> {
    cfg_if! {
        if #[cfg(target_arch = "wasm32")] {
            let url = format_url(file_name);
            let data = reqwest::get(url)
                .await?
                .bytes()
                .await?
                .to_vec();
        } else {
            let path = std::path::Path::new(env!("OUT_DIR"))
                .join("res")
                .join(file_name);
            let data = std::fs::read(path)?;
        }
    }

    Ok(data)
}

pub async fn load_texture(
    file_name: &str,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> anyhow::Result<texture::Texture> {
    let data = load_binary(file_name).await?;
    texture::Texture::from_bytes(device, queue, &data, file_name)
}

pub async fn load_model(
    file_name: &str,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
) -> anyhow::Result<model::Model> {
    let obj_text = load_string(file_name).await.expect("Couldn't load obj file");
    let obj_cursor = Cursor::new(obj_text);
    let mut obj_reader = BufReader::new(obj_cursor);

    let (models, obj_materials) = tobj::load_obj_buf_async(
        &mut obj_reader,
        &tobj::LoadOptions {
            triangulate: true,
            single_index: true,
            ..Default::default()
        },
        |p| async move {
            let mat_text = load_string(&p).await.unwrap();
            tobj::load_mtl_buf(&mut BufReader::new(Cursor::new(mat_text)))
        },
    )
        .await?;

    let mut materials = Vec::new();
    for m in obj_materials? {
        let diffuse_texture = load_texture(&m.diffuse_texture.expect("TEST"), device, queue).await?;
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&diffuse_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&diffuse_texture.sampler),
                },
            ],
            label: None,
        });

        materials.push(model::Material {
            name: m.name,
            diffuse_texture,
            bind_group,
        })
    }

    let meshes = models
        .into_iter()
        .map(|m| {
            let vertices = (0..m.mesh.positions.len() / 3)
                .map(|i| {
                    if m.mesh.normals.is_empty(){
                        model::ModelVertex {
                            position: [
                                m.mesh.positions[i * 3],
                                m.mesh.positions[i * 3 + 1],
                                m.mesh.positions[i * 3 + 2],
                            ],
                            tex_coords: [m.mesh.texcoords[i * 2], 1.0 - m.mesh.texcoords[i * 2 + 1]],
                            normal: [0.0, 0.0, 0.0],
                        }
                    }else{
                        model::ModelVertex {
                            position: [
                                m.mesh.positions[i * 3],
                                m.mesh.positions[i * 3 + 1],
                                m.mesh.positions[i * 3 + 2],
                            ],
                            tex_coords: [m.mesh.texcoords[i * 2], 1.0 - m.mesh.texcoords[i * 2 + 1]],
                            normal: [
                                m.mesh.normals[i * 3],
                                m.mesh.normals[i * 3 + 1],
                                m.mesh.normals[i * 3 + 2],
                            ],
                        }
                    }
                })
                .collect::<Vec<_>>();

            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{:?} Vertex Buffer", file_name)),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{:?} Index Buffer", file_name)),
                contents: bytemuck::cast_slice(&m.mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });

            log::info!("Mesh: {}", m.name);
            model::Mesh {
                name: file_name.to_string(),
                vertex_buffer,
                index_buffer,
                num_elements: m.mesh.indices.len() as u32,
                material: m.mesh.material_id.unwrap_or(0),
            }
        })
        .collect::<Vec<_>>();

    Ok(model::Model { meshes, materials })
}


pub async fn load_model_gltf(
    file_name: &str,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
) -> anyhow::Result<model::Model> {
    let gltf_text = load_string(file_name).await.expect("Couldn't load gltf file");
    print!("{:?}", gltf_text);

    let (gltf, buffers, _) = gltf::import(gltf_text).expect("Failed to load gltf file");

    let mut materials = Vec::new();

    for material in gltf.materials() {
        println!("Looping thru materials");
        let pbr = material.pbr_metallic_roughness();
        let texture_source = &pbr
            .base_color_texture()
            .map(|tex| {
                // println!("Grabbing diffuse tex");
                // dbg!(&tex.texture().source());
                tex.texture().source().source()
            })
            .expect("texture");

        match texture_source {
            gltf::image::Source::View { view, mime_type } => {
                let diffuse_texture = texture::Texture::from_bytes(
                    device,
                    queue,
                    &buffers[view.buffer().index()],
                    file_name,
                ).expect("Couldn't load diffuse");

                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&diffuse_texture.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&diffuse_texture.sampler),
                        },
                    ],
                    label: None,
                });

                materials.push(model::Material {
                    name: material.name().unwrap().parse()?,
                    diffuse_texture,
                    bind_group,
                });
            }

            gltf::image::Source::Uri { uri, mime_type } => {
                println!("Uri: {:?}", uri);
            }
        };
    }

    let mut meshes = Vec::new();

    for scene in gltf.scenes() {
        for node in scene.nodes() {
            println!("Node {}", node.index());
            // dbg!(node);

            if let Some(mesh) = node.mesh() {
                let primitives = mesh.primitives();
                primitives.for_each(|primitive| {
                    // dbg!(primitive);

                    let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

                    let mut vertices = Vec::new();
                    if let Some(vertex_attribute) = reader.read_positions() {
                        vertex_attribute.for_each(|vertex| {
                            // dbg!(vertex);
                            vertices.push(ModelVertex {
                                position: vertex,
                                tex_coords: Default::default(),
                                normal: Default::default(),
                            })
                        });
                    }
                    if let Some(normal_attribute) = reader.read_normals() {
                        let mut normal_index = 0;
                        normal_attribute.for_each(|normal| {
                            // dbg!(normal);
                            vertices[normal_index].normal = normal;

                            normal_index += 1;
                        });
                    }
                    if let Some(tex_coord_attribute) = reader.read_tex_coords(0).map(|v| v.into_f32()) {
                        let mut tex_coord_index = 0;
                        tex_coord_attribute.for_each(|tex_coord| {
                            // dbg!(tex_coord);
                            vertices[tex_coord_index].tex_coords = tex_coord;

                            tex_coord_index += 1;
                        });
                    }

                    let mut indices = Vec::new();
                    if let Some(indices_raw) = reader.read_indices() {
                        // dbg!(indices_raw);
                        indices.append(&mut indices_raw.into_u32().collect::<Vec<u32>>());
                    }
                    // dbg!(indices);

                    // println!("{:#?}", &indices.expect("got indices").data_type());
                    // println!("{:#?}", &indices.expect("got indices").index());
                    // println!("{:#?}", &material);

                    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&format!("{:?} Vertex Buffer", file_name)),
                        contents: bytemuck::cast_slice(&vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&format!("{:?} Index Buffer", file_name)),
                        contents: bytemuck::cast_slice(&indices),
                        usage: wgpu::BufferUsages::INDEX,
                    });

                    meshes.push(model::Mesh {
                        name: file_name.to_string(),
                        vertex_buffer,
                        index_buffer,
                        num_elements: indices.len() as u32,
                        // material: m.mesh.material_id.unwrap_or(0),
                        material: 0,
                    });
                });
            }
        }
    }

    Ok(model::Model {
        meshes,
        materials,
    })
}
