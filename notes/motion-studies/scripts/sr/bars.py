import sys, numpy as np
from PIL import Image
from scipy import ndimage
d=sys.argv[1]
for F in range(int(sys.argv[2]),int(sys.argv[3])+1):
    a=np.asarray(Image.open(f"{d}/{F:04d}.png").convert('RGB')).astype(int)
    m=(a[:,:,1]>140)&(a[:,:,0]<140)&(a[:,:,2]<120)&(a[:,:,1]-a[:,:,0]>60)
    m[:200,:]=False
    lab,n=ndimage.label(m)
    out=[]
    for sl in ndimage.find_objects(lab):
        h=sl[0].stop-sl[0].start; w=sl[1].stop-sl[1].start
        if h>=6 and 8<=w<=60: out.append(((sl[1].start+sl[1].stop)//2, h))
    out.sort()
    print(f"F{F}", ' '.join(f"{x}:{h}" for x,h in out))
