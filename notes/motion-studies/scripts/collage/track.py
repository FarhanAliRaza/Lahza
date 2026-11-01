import sys, numpy as np
from PIL import Image
# usage: track.py dir ref x0 y0 x1 y1 search frames...
d=sys.argv[1]; ref=int(sys.argv[2]); x0,y0,x1,y1=map(int,sys.argv[3:7]); S=int(sys.argv[7]); frames=[int(f) for f in sys.argv[8:]]
ld=lambda i: np.asarray(Image.open(f"{d}/{i:04d}.png").convert('L'),dtype=np.float32)
R=ld(ref)[y0:y1,x0:x1]; R=R-R.mean()
for f in frames:
    F=ld(f); best=None
    for dy in range(-S,S+1):
        for dx in range(-S,S+1):
            C=F[y0+dy:y1+dy,x0+dx:x1+dx]; C=C-C.mean()
            s=(R*C).sum()/np.sqrt((R*R).sum()*(C*C).sum()+1e-9)
            if best is None or s>best[0]: best=(s,dx,dy)
    print(f, "dx=%d dy=%d corr=%.3f"%(best[1],best[2],best[0]))
