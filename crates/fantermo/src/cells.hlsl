// セルの背景・カーソル・グリフを、インスタンスごとの四角形として描く。
// 頂点バッファは使わず、SV_VertexID（0〜3 の三角形ストリップ）から四角形の角を作る。

cbuffer Viewport : register(b0)
{
    float2 viewport_size; // 描画先の大きさ（ピクセル）
    float2 atlas_size;    // グリフアトラスの大きさ（ピクセル）
};

Texture2D atlas : register(t0);
SamplerState atlas_sampler : register(s0);

// kind の値。Rust 側の InstanceKind と揃える。
static const uint KIND_SOLID = 0;
static const uint KIND_GLYPH = 1;

struct Instance
{
    float4 rect : RECT;   // 左上の x, y と幅, 高さ（ピクセル）
    float4 uv : UV;       // アトラスの中の左上の x, y と幅, 高さ（ピクセル）
    float4 color : COLOR; // 直線の RGBA（0〜1）
    uint kind : KIND;
};

struct Fragment
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float4 color : COLOR;
    nointerpolation uint kind : KIND;
};

Fragment vs_main(uint vertex : SV_VertexID, Instance instance)
{
    float2 corner = float2(vertex & 1, vertex >> 1);
    float2 pixel = instance.rect.xy + corner * instance.rect.zw;

    Fragment fragment;
    fragment.position = float4(pixel / viewport_size * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    fragment.uv = (instance.uv.xy + corner * instance.uv.zw) / atlas_size;
    fragment.color = instance.color;
    fragment.kind = instance.kind;
    return fragment;
}

// 出力は乗算済みのアルファ。
float4 ps_main(Fragment fragment) : SV_Target
{
    float alpha = fragment.color.a;
    if (fragment.kind == KIND_GLYPH)
    {
        // アトラスは白い文字を乗算済みのアルファで持つので、覆う割合はアルファに入っている。
        alpha *= atlas.Sample(atlas_sampler, fragment.uv).a;
    }
    return float4(fragment.color.rgb * alpha, alpha);
}
