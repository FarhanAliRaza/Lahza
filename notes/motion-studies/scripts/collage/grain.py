import sys, numpy as np
from PIL import Image
x0,y0,x1,y1=map(int,sys.argv[1].split(',')); fr=[int(f) for f in sys.argv[3:]]; d=sys.argv[2]
prev=None
for f in fr:
    a=np.asarray(Image.open(f"{d}/{f:04d}.png").convert('RGB'),dtype=np.float32)[y0:y1,x0:x1]
    L=a.mean(2)
    # high-pass: subtract 9x9 box mean
    from numpy.lib.stride_tricks import sliding_window_view as sw
    k=9; m=sw(np.pad(L,k//2,mode='edge'),(k,k)).mean((2,3))
    hp=L-m
    s=f"{f}: meanRGB={a.reshape(-1,3).mean(0).round(1)} std={L.std():.2f} hp_std={hp.std():.2f}"
    if prev is not None: s+=f" tdiff={np.abs(L-prev).mean():.2f}"
    prev=L; print(s)
