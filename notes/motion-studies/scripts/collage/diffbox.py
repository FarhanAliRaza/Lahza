import sys, numpy as np
from PIL import Image
d=sys.argv[1]; fr=[int(x) for x in sys.argv[2:]]
ld=lambda i: np.asarray(Image.open(f"{d}/{i:04d}.png").convert('L'),dtype=np.int16)
for a,b in zip(fr,fr[1:]):
    D=np.abs(ld(b)-ld(a))>20
    ys,xs=np.nonzero(D)
    if len(xs)==0: print(a,b,"none"); continue
    # row/col histograms coarse
    print(a,b,"n=%d bbox x%d-%d y%d-%d"%(len(xs),xs.min(),xs.max(),ys.min(),ys.max()), "rows(100px):",np.bincount(ys//100,minlength=13).tolist())
