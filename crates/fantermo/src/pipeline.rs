//! 背景画像とセルを四角形のインスタンスとして描く Direct3D 11 のパイプライン（`cells.hlsl`）。

use windows::{
    Win32::Graphics::{
        Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
        Direct3D11::{
            D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_VERTEX_BUFFER, D3D11_BLEND_DESC,
            D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE, D3D11_BLEND_OP_ADD, D3D11_BUFFER_DESC,
            D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_COMPARISON_NEVER, D3D11_CPU_ACCESS_WRITE,
            D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_FILTER_MIN_MAG_MIP_POINT,
            D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_INSTANCE_DATA, D3D11_MAP_WRITE_DISCARD,
            D3D11_MAPPED_SUBRESOURCE, D3D11_RENDER_TARGET_BLEND_DESC, D3D11_SAMPLER_DESC,
            D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE_ADDRESS_MODE, D3D11_TEXTURE_ADDRESS_WRAP,
            D3D11_USAGE_DYNAMIC, D3D11_VIEWPORT, ID3D11BlendState, ID3D11Buffer, ID3D11Device,
            ID3D11DeviceContext, ID3D11InputLayout, ID3D11PixelShader, ID3D11RenderTargetView,
            ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11VertexShader,
        },
        Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R32G32B32A32_FLOAT},
    },
    core::{PCSTR, s},
};

use crate::shader::{self, CompileError};

/// 四角形一つの頂点の数（三角形ストリップ）。
const QUAD_VERTICES: u32 = 4;

/// インスタンスの種類。`cells.hlsl` の KIND_* と揃える。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum InstanceKind {
    /// 塗りつぶし（背景、背景画像の上に重ねる背景色、カーソル）。
    Solid = 0,
    /// アトラスのグリフ。
    Glyph = 1,
    /// 背景画像。`uv` は画像の幅と高さを 1 とし、範囲の外は端の色。
    Image = 2,
    /// 背景画像を繰り返す。`uv` は画像の幅と高さを 1 とする。
    ImageRepeat = 3,
}

/// シェーダに渡す四角形一つ。`cells.hlsl` の Instance と並びを揃える。
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Instance {
    /// 左上の x, y と幅, 高さ（ピクセル）。
    pub rect: [f32; 4],
    /// アトラスの中の左上の x, y と幅, 高さ（ピクセル）。
    pub uv: [f32; 4],
    /// 直線の RGBA（0〜1）。
    pub color: [f32; 4],
    pub kind: InstanceKind,
    pub _padding: [u32; 3],
}

/// `cells.hlsl` の Viewport と並びを揃える。
#[derive(Clone, Copy)]
#[repr(C)]
struct ViewportConstants {
    viewport_size: [f32; 2],
    atlas_size: [f32; 2],
}

pub struct CellPipeline {
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    input_layout: ID3D11InputLayout,
    constants: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    /// 背景画像のサンプラ。範囲の外を端の色にするものと、繰り返すもの。
    image_samplers: [ID3D11SamplerState; 2],
    blend: ID3D11BlendState,
    instances: Option<ID3D11Buffer>,
    /// `instances` に入るインスタンスの数。
    capacity: usize,
}

#[derive(Debug)]
pub enum PipelineError {
    Compile(CompileError),
    Windows(windows::core::Error),
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineError::Compile(e) => e.fmt(f),
            PipelineError::Windows(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for PipelineError {}

impl From<CompileError> for PipelineError {
    fn from(e: CompileError) -> Self {
        PipelineError::Compile(e)
    }
}

impl From<windows::core::Error> for PipelineError {
    fn from(e: windows::core::Error) -> Self {
        PipelineError::Windows(e)
    }
}

impl CellPipeline {
    pub fn new(device: &ID3D11Device) -> Result<Self, PipelineError> {
        let vertex_code = shader::compile(
            shader::CELLS_SOURCE,
            shader::CELLS_VERTEX_ENTRY,
            shader::VERTEX_TARGET,
        )?;
        let pixel_code = shader::compile(
            shader::CELLS_SOURCE,
            shader::CELLS_PIXEL_ENTRY,
            shader::PIXEL_TARGET,
        )?;
        let vertex_bytes = shader::blob_bytes(&vertex_code);

        let mut vertex_shader = None;
        let mut pixel_shader = None;
        // SAFETY: 二つのバイト列は D3DCompile が出したシェーダで、出力先は書き込める。
        unsafe {
            device.CreateVertexShader(vertex_bytes, None, Some(&mut vertex_shader))?;
            device.CreatePixelShader(
                shader::blob_bytes(&pixel_code),
                None,
                Some(&mut pixel_shader),
            )?;
        }

        let elements = [
            instance_element(s!("RECT"), DXGI_FORMAT_R32G32B32A32_FLOAT, 0),
            instance_element(s!("UV"), DXGI_FORMAT_R32G32B32A32_FLOAT, 16),
            instance_element(s!("COLOR"), DXGI_FORMAT_R32G32B32A32_FLOAT, 32),
            instance_element(s!("KIND"), DXGI_FORMAT_R32_UINT, 48),
        ];
        let mut input_layout = None;
        // SAFETY: elements は Instance の並びと、頂点シェーダの入力と揃えてある。
        unsafe { device.CreateInputLayout(&elements, vertex_bytes, Some(&mut input_layout))? };

        let constants = create_buffer(
            device,
            size_of::<ViewportConstants>(),
            D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        )?;

        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        // SAFETY: sampler_desc は初期化済みで、sampler は書き込める。
        unsafe { device.CreateSamplerState(&sampler_desc, Some(&mut sampler))? };
        let image_sampler = |address: D3D11_TEXTURE_ADDRESS_MODE| {
            let desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: address,
                AddressV: address,
                AddressW: address,
                ..sampler_desc
            };
            let mut sampler = None;
            // SAFETY: desc は初期化済みで、sampler は書き込める。
            unsafe { device.CreateSamplerState(&desc, Some(&mut sampler))? };
            sampler.ok_or_else(windows::core::Error::empty)
        };
        let image_samplers = [
            image_sampler(D3D11_TEXTURE_ADDRESS_CLAMP)?,
            image_sampler(D3D11_TEXTURE_ADDRESS_WRAP)?,
        ];

        // ピクセルシェーダは乗算済みのアルファを出す。
        let mut blend_desc = D3D11_BLEND_DESC::default();
        blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: true.into(),
            SrcBlend: D3D11_BLEND_ONE,
            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_ONE,
            DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        let mut blend = None;
        // SAFETY: blend_desc は初期化済みで、blend は書き込める。
        unsafe { device.CreateBlendState(&blend_desc, Some(&mut blend))? };

        let missing = windows::core::Error::empty;
        Ok(Self {
            vertex_shader: vertex_shader.ok_or_else(missing)?,
            pixel_shader: pixel_shader.ok_or_else(missing)?,
            input_layout: input_layout.ok_or_else(missing)?,
            constants,
            sampler: sampler.ok_or_else(missing)?,
            image_samplers,
            blend: blend.ok_or_else(missing)?,
            instances: None,
            capacity: 0,
        })
    }

    /// `instances` を `target` に並べた順に描く。背景画像のインスタンスは `image` を描く。
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        target: &ID3D11RenderTargetView,
        viewport_size: (u32, u32),
        atlas: &ID3D11ShaderResourceView,
        atlas_size: (u32, u32),
        image: Option<&ID3D11ShaderResourceView>,
        instances: &[Instance],
    ) -> windows::core::Result<()> {
        if instances.is_empty() {
            return Ok(());
        }
        self.reserve(device, instances.len())?;
        let Some(instance_buffer) = &self.instances else {
            return Ok(());
        };
        write_buffer(context, instance_buffer, instances)?;
        write_buffer(
            context,
            &self.constants,
            &[ViewportConstants {
                viewport_size: [viewport_size.0 as f32, viewport_size.1 as f32],
                atlas_size: [atlas_size.0 as f32, atlas_size.1 as f32],
            }],
        )?;

        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: viewport_size.0 as f32,
            Height: viewport_size.1 as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        let stride = size_of::<Instance>() as u32;
        let offset = 0u32;
        // SAFETY: どのオブジェクトも同じデバイスで作ったもので、配列はこの呼び出しの間は生きている。
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            context.OMSetBlendState(&self.blend, None, u32::MAX);
            context.RSSetViewports(Some(&[viewport]));
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            context.IASetInputLayout(&self.input_layout);
            context.IASetVertexBuffers(
                0,
                1,
                Some(&Some(instance_buffer.clone())),
                Some(&stride),
                Some(&offset),
            );
            context.VSSetShader(&self.vertex_shader, None);
            context.VSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            context.PSSetShader(&self.pixel_shader, None);
            context.PSSetShaderResources(0, Some(&[Some(atlas.clone()), image.cloned()]));
            let [image_clamp, image_wrap] = &self.image_samplers;
            context.PSSetSamplers(
                0,
                Some(&[
                    Some(self.sampler.clone()),
                    Some(image_clamp.clone()),
                    Some(image_wrap.clone()),
                ]),
            );
            context.DrawInstanced(QUAD_VERTICES, instances.len() as u32, 0, 0);
        }
        Ok(())
    }

    /// インスタンスのバッファを、少なくとも `count` 個入る大きさにする。
    fn reserve(&mut self, device: &ID3D11Device, count: usize) -> windows::core::Result<()> {
        if count <= self.capacity {
            return Ok(());
        }
        let capacity = count.next_power_of_two();
        self.instances = Some(create_buffer(
            device,
            capacity * size_of::<Instance>(),
            D3D11_BIND_VERTEX_BUFFER.0 as u32,
        )?);
        self.capacity = capacity;
        Ok(())
    }
}

fn instance_element(semantic: PCSTR, format: DXGI_FORMAT, offset: u32) -> D3D11_INPUT_ELEMENT_DESC {
    D3D11_INPUT_ELEMENT_DESC {
        SemanticName: semantic,
        SemanticIndex: 0,
        Format: format,
        InputSlot: 0,
        AlignedByteOffset: offset,
        InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
        InstanceDataStepRate: 1,
    }
}

/// CPU から毎フレーム書き込むバッファ。
fn create_buffer(
    device: &ID3D11Device,
    size: usize,
    bind_flags: u32,
) -> windows::core::Result<ID3D11Buffer> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: size as u32,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: bind_flags,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        ..Default::default()
    };
    let mut buffer = None;
    // SAFETY: desc は初期化済みで、buffer は書き込める。
    unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer))? };
    buffer.ok_or_else(windows::core::Error::empty)
}

/// `buffer` の中身を `items` で置き換える。`buffer` は `items` が入る大きさであること。
fn write_buffer<T: Copy>(
    context: &ID3D11DeviceContext,
    buffer: &ID3D11Buffer,
    items: &[T],
) -> windows::core::Result<()> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: buffer は CPU から書き込める動的なバッファで、mapped は書き込める。
    unsafe { context.Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))? };
    // SAFETY: Map が成功したので、pData から buffer の大きさ分を書き込める。呼び出し元が大きさを保証する。
    unsafe {
        std::ptr::copy_nonoverlapping(items.as_ptr(), mapped.pData.cast::<T>(), items.len());
        context.Unmap(buffer, 0);
    }
    Ok(())
}
