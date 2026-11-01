import sys
from PIL import Image, ImageChops, ImageStat
prev=None; out=[]
for n in range(0,1019):
    im=Image.open(f'all/{n:04d}.png').convert('L').resize((160,90))
    if prev is not None:
        out.append((n,ImageStat.Stat(ImageChops.difference(im,prev)).mean[0]))
    prev=im
open('motion.txt','w').write('\n'.join(f'{n} {v:.2f}' for n,v in out))
