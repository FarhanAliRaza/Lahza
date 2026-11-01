# usage: track_text.py dir F0 F1 x0 y0 x1 y1 thr [mode]  -> bbox of bright pixels (all channels > thr)
import sys
from PIL import Image
d=sys.argv[1]; a,b,x0,y0,x1,y1,thr=map(int,sys.argv[2:9]); mode=sys.argv[9] if len(sys.argv)>9 else 'bright'
for F in range(a,b+1):
    im=Image.open(f"{d}/{F:04d}.png").convert('RGB'); px=im.load()
    if mode=='bright': ok=lambda c: min(c)>thr
    else: ok=lambda c: max(c)<thr
    pts=[(x,y) for y in range(y0,y1) for x in range(x0,x1) if ok(px[x,y])]
    if not pts: print(F,'none'); continue
    xs=[p[0] for p in pts]; ys=[p[1] for p in pts]
    print(f"F{F} t={F*0.04:.2f} bbox=({min(xs)},{min(ys)},{max(xs)},{max(ys)}) w={max(xs)-min(xs)} h={max(ys)-min(ys)} cx={(min(xs)+max(xs))/2:.0f} cy={(min(ys)+max(ys))/2:.0f} n={len(pts)}")
