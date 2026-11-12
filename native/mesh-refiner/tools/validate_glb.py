#!/usr/bin/env python3
"""Structural GLB/accessor validation and source-output asset parity checks."""
import argparse, hashlib, json, math, struct
from pathlib import Path
import numpy as np

def parse(path):
    b=Path(path).read_bytes()
    if len(b)<20 or b[:4]!=b'glTF': raise ValueError(f'{path}: bad magic')
    magic,version,total=struct.unpack_from('<III',b,0)
    if version!=2 or total!=len(b): raise ValueError(f'{path}: bad version or total length')
    off=12;doc=None;blob=None
    while off<len(b):
        if off+8>len(b): raise ValueError(f'{path}: truncated chunk header')
        n,t=struct.unpack_from('<II',b,off);off+=8
        if n%4: raise ValueError(f'{path}: chunk not 4-byte aligned')
        if off+n>len(b): raise ValueError(f'{path}: truncated chunk')
        chunk=b[off:off+n];off+=n
        if t==0x4e4f534a: doc=json.loads(chunk.rstrip(b' \0'))
        if t==0x004e4942: blob=chunk
    if doc is None or blob is None: raise ValueError(f'{path}: missing JSON or BIN')
    if len(blob)<doc['buffers'][0]['byteLength']: raise ValueError(f'{path}: BIN shorter than declared buffer')
    for i,v in enumerate(doc.get('bufferViews',[])):
        start=v.get('byteOffset',0);end=start+v['byteLength']
        if v.get('buffer',0)!=0 or start<0 or end>len(blob): raise ValueError(f'{path}: bufferView {i} out of bounds')
    # This focused validator supports ordinary tightly/strided arrays and all glTF scalar widths.
    ct={5120:('i1',1),5121:('u1',1),5122:('<i2',2),5123:('<u2',2),5125:('<u4',4),5126:('<f4',4)}
    widths={'SCALAR':1,'VEC2':2,'VEC3':3,'VEC4':4,'MAT2':4,'MAT3':9,'MAT4':16}
    decoded=[]
    for i,a in enumerate(doc.get('accessors',[])):
        if 'sparse' in a: raise ValueError(f'{path}: sparse accessor {i} unsupported by local validator')
        if 'bufferView' not in a: raise ValueError(f'{path}: accessor {i} has no bufferView')
        v=doc['bufferViews'][a['bufferView']];dt,cb=ct[a['componentType']];w=widths[a['type']];stride=v.get('byteStride',cb*w);start=v.get('byteOffset',0)+a.get('byteOffset',0)
        if start+max(0,a['count']-1)*stride+cb*w > v.get('byteOffset',0)+v['byteLength']: raise ValueError(f'{path}: accessor {i} exceeds view')
        arr=np.ndarray((a['count'],w),dtype=dt,buffer=blob,offset=start,strides=(stride,cb));
        if a['componentType']==5126 and not np.isfinite(arr).all(): raise ValueError(f'{path}: accessor {i} has non-finite floats')
        if a['type']=='SCALAR' and a['componentType'] in (5121,5123,5125):
            if arr.size and (arr.min()<0): raise ValueError(f'{path}: negative index')
        decoded.append(np.array(arr,copy=True))
    meshes=doc.get('meshes',[])
    if not meshes: raise ValueError(f'{path}: no meshes')
    tri_counts=[]
    for mi,m in enumerate(meshes):
        for pi,p in enumerate(m['primitives']):
            if p.get('mode',4)!=4: raise ValueError(f'{path}: primitive {mi}/{pi} is not TRIANGLES')
            ind=decoded[p['indices']].reshape(-1)
            if len(ind)%3: raise ValueError(f'{path}: primitive index count is not triangle-aligned')
            n=decoded[p['attributes']['POSITION']].shape[0]
            if len(ind) and int(ind.max())>=n: raise ValueError(f'{path}: primitive index out of range')
            for name,aid in p['attributes'].items():
                if decoded[aid].shape[0]!=n: raise ValueError(f'{path}: attribute {name} count mismatch')
            tri_counts.append(len(ind)//3)
    image_hashes=[]
    for im in doc.get('images',[]):
        if 'bufferView' in im:
            v=doc['bufferViews'][im['bufferView']];s=v.get('byteOffset',0);image_hashes.append(hashlib.sha256(blob[s:s+v['byteLength']]).hexdigest())
        else:image_hashes.append(None)
    return {'doc':doc,'blob':blob,'decoded':decoded,'triangles':tri_counts,'images':image_hashes,'bytes':len(b)}

def main():
    ap=argparse.ArgumentParser();ap.add_argument('output');ap.add_argument('--source');args=ap.parse_args()
    o=parse(args.output);report={'output':str(args.output),'file_bytes':o['bytes'],'triangles':o['triangles'],'materials':len(o['doc'].get('materials',[])),'embedded_image_sha256':o['images'],'structural_checks':'passed'}
    if args.source:
        s=parse(args.source);report['source']=str(args.source);report['source_triangles']=s['triangles'];report['textures_byte_identical']=s['images']==o['images'];report['materials_equal']=s['doc'].get('materials')==o['doc'].get('materials');report['texture_image_count_equal']=len(s['images'])==len(o['images'])
        if not report['textures_byte_identical'] or not report['materials_equal']:raise ValueError('material or embedded texture parity check failed')
        for key in ('TEXCOORD_0','NORMAL'):
            if key not in s['doc']['meshes'][0]['primitives'][0]['attributes'] or key not in o['doc']['meshes'][0]['primitives'][0]['attributes']: raise ValueError(f'{key} missing')
        report['uv_and_normal_attributes_present']=True
    print(json.dumps(report,indent=2))
if __name__=='__main__':main()
