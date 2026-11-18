import { CheckIcon, CubeIcon, ImageSquareIcon } from '@phosphor-icons/react';
import { useEffect, useRef, useState } from 'react';
import { cn } from '@/lib/utils';
import { assetThumbnail } from './asset-thumbnails';
import type { Asset } from './types';

const compactCount = new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 });

export function AssetGalleryCard({
  asset, selected, loadPreview, onSelect,
}: {
  asset: Asset;
  selected: boolean;
  loadPreview: (asset: Asset, signal: AbortSignal) => Promise<Blob>;
  onSelect: () => void;
}) {
  const card = useRef<HTMLButtonElement>(null);
  const [visible, setVisible] = useState(false);
  const [image, setImage] = useState<{ hash: string; url: string } | null>(null);
  const Icon = asset.kind === 'mesh' ? CubeIcon : ImageSquareIcon;
  const triangles = Number(asset.metadata.triangles);

  useEffect(() => {
    if (visible || !card.current) return;
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    });
    observer.observe(card.current);
    return () => observer.disconnect();
  }, [visible]);

  useEffect(() => {
    if (!visible) return;
    const controller = new AbortController();
    let url: string | undefined;
    setImage(null);
    void assetThumbnail(asset, loadPreview, controller.signal)
      .then(blob => {
        if (!blob || controller.signal.aborted) return;
        url = URL.createObjectURL(blob);
        setImage({ hash: asset.sha256, url });
      })
      .catch(() => undefined);
    return () => {
      controller.abort();
      if (url) URL.revokeObjectURL(url);
    };
  }, [asset.id, asset.kind, asset.sha256, loadPreview, visible]);

  return (
    <button type="button" ref={card} onClick={onSelect} aria-pressed={selected}
      className={cn(
        'group grid min-w-0 gap-0 overflow-hidden rounded-lg border bg-card text-left text-card-foreground transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring',
        selected ? 'border-primary bg-accent' : 'hover:border-muted-foreground/60 hover:bg-muted',
      )}>
      <span className="relative grid aspect-square w-full min-w-0 place-items-center overflow-hidden bg-neutral-800">
        {image?.hash === asset.sha256
          ? <img src={image.url} alt="" data-asset-thumbnail="" decoding="async" className="absolute inset-0 size-full object-contain" />
          : <Icon className="size-7 text-muted-foreground/60" aria-hidden="true" />}
        <span aria-hidden="true" className="absolute bottom-1.5 left-1.5 rounded bg-black/70 px-1.5 py-0.5 text-[10px] leading-none font-medium text-white">
          {asset.kind === 'mesh' ? '3D' : 'IMG'}
        </span>
        {selected && <span className="absolute top-1.5 right-1.5 grid size-5 place-items-center rounded-full bg-primary text-primary-foreground">
          <CheckIcon className="size-3" weight="bold" aria-hidden="true" />
        </span>}
      </span>
      <span className="grid min-w-0 gap-0.5 px-2 py-1.5">
        <span dir="rtl" className="block min-w-0 truncate text-left text-xs font-medium" title={asset.name}><bdi>{asset.name}</bdi></span>
        <span className="block min-w-0 truncate text-[10px] tabular-nums text-muted-foreground">
          {asset.kind === 'image' ? 'Image' : Number.isFinite(triangles) ? `${compactCount.format(triangles)} tris` : 'Mesh'}
          {' · '}{asset.source_asset_id ? 'Result' : 'Original'}
        </span>
      </span>
    </button>
  );
}
