import sys
from PIL import Image
from collections import Counter
for f in sys.argv[1:]:
    im=Image.open(f).convert('RGB').resize((320,180),Image.NEAREST)
    c=Counter(im.getdata())
    # quantize to 4 bits to merge AA
    q=Counter()
    rep={}
    for col,n in c.items():
        k=tuple(v>>3 for v in col); q[k]+=n
        if k not in rep or c[rep[k]]<n: rep[k]=col
    tot=sum(q.values())
    print(f.split('/')[-1], ' '.join('#%02x%02x%02x:%.1f%%'%(*rep[k],100*n/tot) for k,n in q.most_common(10) if n/tot>0.004))
