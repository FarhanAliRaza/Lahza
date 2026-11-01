import sys, colorsys
from PIL import Image
R=sys.argv[1]
for n,label in [(20,"shot1 gradient"),(35,"shot1 gradient b"),(60,"line drawing"),(175,"BlackLine arc"),(195,"sphere+orange line"),(320,"break free"),(345,"closeup"),(415,"rings"),(450,"tubes"),(118,"is ring")]:
    im=Image.open(f'{R}/{n:03d}.jpg').convert('RGB').resize((240,300)); px=im.load()
    buckets={}
    for y in range(300):
        for x in range(240):
            r,g,b=px[x,y]; h,s,v=colorsys.rgb_to_hsv(r/255,g/255,b/255)
            if v<0.25: continue
            key=int(h*12)%12 if s>0.3 else 'grey'
            buckets.setdefault(key,[]).append((r,g,b,v,s))
    out=[]
    for k,l in sorted(buckets.items(),key=lambda kv:-len(kv[1])):
        if len(l)<150: continue
        l.sort(key=lambda t:t[3]); med=l[len(l)//2]; hi=l[int(len(l)*0.95)]
        out.append(f"{k}:{len(l)} med #{med[0]:02x}{med[1]:02x}{med[2]:02x} hi #{hi[0]:02x}{hi[1]:02x}{hi[2]:02x}")
    print(f"{label:18s}", " | ".join(out[:5]))
