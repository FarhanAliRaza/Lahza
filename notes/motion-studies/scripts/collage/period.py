import sys, numpy as np
from PIL import Image
a=np.asarray(Image.open(sys.argv[1]).convert('L'),dtype=np.float32)
x0,y0,x1,y1=map(int,sys.argv[2].split(','))
r=a[y0:y1,x0:x1]; r=r-r.mean()
F=np.abs(np.fft.fft2(r))**2; F[0,0]=0
h,w=r.shape
fy=np.fft.fftfreq(h)[:,None]; fx=np.fft.fftfreq(w)[None,:]
f=np.sqrt(fx**2+fy**2); F[f<0.05]=0  # drop low-frequency content
idx=np.argsort(F.ravel())[::-1][:6]
for i in idx:
    yy,xx=np.unravel_index(i,F.shape)
    print("fx=%.3f fy=%.3f period=%.2fpx angle=%.0fdeg"%(fx[0,xx],fy[yy,0],1/f[yy,xx],np.degrees(np.arctan2(fy[yy,0],fx[0,xx]))))
