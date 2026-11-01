import sys, numpy as np
from PIL import Image
from scipy import ndimage
d=sys.argv[1]
for F in range(int(sys.argv[2]),int(sys.argv[3])+1):
    a=np.asarray(Image.open(f"{d}/{F:04d}.png").convert('RGB')).astype(int)
    g=(a[:,:,1]>170)&(a[:,:,0]<170)&(a[:,:,1]-a[:,:,2]>30)
    w=(a.min(axis=2)>185)
    def big(m):
        lab,n=ndimage.label(m)
        if n==0: return None
        sizes=ndimage.sum(m,lab,range(1,n+1)); i=int(np.argmax(sizes))+1
        if sizes[i-1]<300: return None
        sl=ndimage.find_objects(lab)[i-1]; return (sl[1].start,sl[0].start,sl[1].stop,sl[0].stop,int(sizes[i-1]))
    gb=big(g); wb=big(w)
    s=f"F{F}"
    if gb: s+=f" green=({gb[0]},{gb[1]},{gb[2]},{gb[3]}) w={gb[2]-gb[0]} h={gb[3]-gb[1]} area={gb[4]}"
    if wb: s+=f" | white=({wb[0]},{wb[1]},{wb[2]},{wb[3]}) w={wb[2]-wb[0]} h={wb[3]-wb[1]}"
    print(s)
