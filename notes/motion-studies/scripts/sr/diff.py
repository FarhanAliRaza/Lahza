import sys, os
from PIL import Image, ImageChops, ImageStat
d=sys.argv[1]; files=sorted(os.listdir(d))
prev=None; out=[]
for i,f in enumerate(files):
    im=Image.open(os.path.join(d,f)).convert('L').resize((80,45))
    if prev is not None:
        df=ImageStat.Stat(ImageChops.difference(im,prev)).mean[0]
        h1=im.histogram(); h0=prev.histogram()
        hd=sum(abs(a-b) for a,b in zip(h1,h0))/(2*80*45)
        out.append((i,df,hd))
    prev=im
with open(sys.argv[2],'w') as fo:
    for i,df,hd in out: fo.write(f"{i} {(i+1)*0.04:.2f} {df:.1f} {hd:.2f}\n")
