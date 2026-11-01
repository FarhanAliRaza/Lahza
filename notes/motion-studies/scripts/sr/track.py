# usage: track.py dir Fref x y w h F0 F1 search [chain]
# NCC template tracking on grayscale; prints position of patch top-left per frame, plus sharpness (laplacian var) of the patch
import sys, numpy as np
from PIL import Image
from numpy.lib.stride_tricks import sliding_window_view as swv
d=sys.argv[1]; Fr,x,y,w,h,F0,F1,R=map(int,sys.argv[2:10]); chain=len(sys.argv)>10
def g(F): return np.asarray(Image.open(f"{d}/{F:04d}.png").convert('L'),dtype=np.float32)
def ncc_search(img,T,cx,cy,R):
    H,W=img.shape; th,tw=T.shape
    x0=max(0,cx-R); y0=max(0,cy-R); x1=min(W-tw,cx+R); y1=min(H-th,cy+R)
    reg=img[y0:y1+th,x0:x1+tw]
    win=swv(reg,(th,tw))[::2,::2]
    Tz=(T-T.mean())/(T.std()+1e-6)
    m=win.mean(axis=(2,3),keepdims=True); s=win.std(axis=(2,3),keepdims=True)+1e-6
    c=((win-m)/s*Tz).mean(axis=(2,3))
    iy,ix=np.unravel_index(np.argmax(c),c.shape); bx,by=x0+ix*2,y0+iy*2
    # refine +-2
    best=(-9,bx,by)
    for yy in range(max(0,by-2),min(H-th,by+3)):
        for xx in range(max(0,bx-2),min(W-tw,bx+3)):
            p=img[yy:yy+th,xx:xx+tw]; v=(((p-p.mean())/(p.std()+1e-6))*Tz).mean()
            if v>best[0]: best=(v,xx,yy)
    return best
ref=g(Fr); T=ref[y:y+h,x:x+w]
order=list(range(Fr,F1+1))+list(range(Fr-1,F0-1,-1))
pos={Fr:(1.0,x,y)}; 
for direction in (range(Fr+1,F1+1),range(Fr-1,F0-1,-1)):
    cx,cy=x,y; Tc=T
    for F in direction:
        img=g(F); v,bx,by=ncc_search(img,Tc,cx,cy,R); pos[F]=(v,bx,by); cx,cy=bx,by
        if chain: Tc=img[by:by+h,bx:bx+w]
for F in sorted(pos):
    v,bx,by=pos[F]; img=g(F); p=img[by:by+h,bx:bx+w]
    lap=(p[1:-1,1:-1]*4-p[:-2,1:-1]-p[2:,1:-1]-p[1:-1,:-2]-p[1:-1,2:]).var()
    print(f"F{F} t={F*0.04:.2f} x={bx} y={by} ncc={v:.2f} sharp={lap:.0f}")
