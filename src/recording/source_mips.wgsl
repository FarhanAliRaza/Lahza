struct MipParams { previous: vec4<u32>, next: vec4<u32>, crop: vec4<u32> }
var<storage, read> b_mip_params: MipParams;
var<storage, read> b_mip_source: array<u32>;
var<storage, read_write> b_mip_output: array<u32>;
fn read_pixel(x: u32,y: u32) -> vec4<u32> {
 let previous=b_mip_params.previous;
 var position=min(vec2<u32>(x,y),previous.xy-vec2<u32>(1u));
 if previous.w==0u {position=clamp(position,b_mip_params.crop.xy,b_mip_params.crop.zw);}
 let index=position.y*previous.x+position.x;
 var p=0u;
 if previous.w==0u {p=b_mip_source[index];}else{p=b_mip_output[previous.z+index];}
 return vec4<u32>(p,p>>8u,p>>16u,p>>24u)&vec4<u32>(255u);
}
@compute @workgroup_size(8,8,1)
fn downsample(@builtin(global_invocation_id) id: vec3<u32>) {
 let next=b_mip_params.next;if any(id.xy>=next.xy){return;}
 var sum=vec4<u32>(0u);
 for(var y=0u;y<2u;y++){for(var x=0u;x<2u;x++){
 let p=read_pixel(id.x*2u+x,id.y*2u+y);sum+=vec4<u32>(p.xyz*p.w,p.w);
 }}
 let rgb=(sum.xyz+vec3<u32>(sum.w/2u))/max(sum.w,1u);let alpha=(sum.w+2u)/4u;
 b_mip_output[next.z+id.y*next.x+id.x]=rgb.x|(rgb.y<<8u)|(rgb.z<<16u)|(alpha<<24u);
}
