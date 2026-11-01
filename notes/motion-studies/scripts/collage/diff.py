import sys,os
from PIL import Image, ImageChops, ImageStat
d=sys.argv[1]; fs=sorted(os.listdir(d))
prev=None
out=open(sys.argv[2],'w')
for i,f in enumerate(fs):
    im=Image.open(os.path.join(d,f))
    if prev is not None:
        m=ImageStat.Stat(ImageChops.difference(im,prev)).mean[0]
    else: m=0
    out.write(f"{i}\t{i/30:.3f}\t{m:.2f}\n")
    prev=im
