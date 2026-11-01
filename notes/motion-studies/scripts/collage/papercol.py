import sys, numpy as np
from PIL import Image
for p in sys.argv[2:]:
    a=np.asarray(Image.open(p).convert('RGB'),dtype=np.float32)
    x0,y0,x1,y1=map(int,sys.argv[1].split(','))
    r=a[y0:y1,x0:x1].reshape(-1,3); L=r.mean(1)
    sel=r[(L>np.percentile(L,60))&(L<np.percentile(L,95))]
    m=sel.mean(0); print(p, "paper RGB", m.round(1), "hex #%02X%02X%02X"%tuple(int(v) for v in m))
