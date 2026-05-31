import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Input } from '@/components/ui/input'
import { useScheduling, useUpdateScheduling } from '@/hooks/use-credentials'
import type { SchedulingMode, UpdateSchedulingPayload } from '@/api/credentials'
import { extractErrorMessage } from '@/lib/utils'

const MODE_LABELS: Record<SchedulingMode, string> = {
  affinity: '会话亲和（推荐）',
  balanced: '负载均衡',
  priority: '优先级固定',
}

const MODE_HINTS: Record<SchedulingMode, string> = {
  affinity: '同会话锁定同账号，最大化 Kiro 前缀缓存命中；不同会话按最久未用账号分散。',
  balanced: '每个请求选成功次数最少的账号 —— 会把同会话打散到多账号，缓存命中差。',
  priority: '固定使用最高优先级账号，缓存友好但易把负载全压一个账号触发 429。',
}

export function SchedulingPanel() {
  const { data, isLoading } = useScheduling()
  const { mutate: update, isPending } = useUpdateScheduling()

  // 本地编辑态（失焦/切换时提交），避免每次按键都打 PUT
  const [mode, setMode] = useState<SchedulingMode>('affinity')
  const [k, setK] = useState('3')
  const [cooldown, setCooldown] = useState('300')
  const [ttl, setTtl] = useState('1800')
  // 感知缓存放大：开关 + 比例（百分数输入，0-100）
  const [cacheOn, setCacheOn] = useState(false)
  const [cachePct, setCachePct] = useState('95')

  useEffect(() => {
    if (!data) return
    setMode(data.mode)
    setK(String(data.affinityPromoteThreshold))
    setCooldown(String(data.cooldownSecs))
    setTtl(String(data.affinityMapTtlSecs))
    const ratio = data.perceivedCacheHitRatio
    setCacheOn(ratio !== null)
    if (ratio !== null) setCachePct(String(Math.round(ratio * 100)))
  }, [data])

  const commit = (patch: UpdateSchedulingPayload, label: string) => {
    update(patch, {
      onSuccess: () => toast.success(`${label}已保存`),
      onError: (error) => toast.error(`${label}保存失败: ${extractErrorMessage(error)}`),
    })
  }

  const commitMode = (next: SchedulingMode) => {
    setMode(next)
    if (next !== data?.mode) commit({ mode: next }, '调度模式')
  }

  const commitNumber = (
    raw: string,
    field: keyof UpdateSchedulingPayload,
    current: number | undefined,
    label: string,
  ) => {
    const n = Number(raw)
    if (!Number.isFinite(n) || n <= 0) {
      toast.error(`${label}必须是正整数`)
      return
    }
    if (n === current) return
    commit({ [field]: Math.floor(n) } as UpdateSchedulingPayload, label)
  }

  // 切换感知缓存放大开关
  const commitCacheToggle = (on: boolean) => {
    setCacheOn(on)
    if (on) {
      const pct = Number(cachePct)
      const ratio = Number.isFinite(pct) ? Math.min(Math.max(pct, 0), 100) / 100 : 0.95
      commit({ perceivedCacheHitRatio: ratio }, '缓存放大比例')
    } else {
      commit({ disablePerceivedCache: true }, '缓存放大')
    }
  }

  // 提交感知缓存放大比例（百分数 0-100 → 0-1）
  const commitCachePct = () => {
    if (!cacheOn) return
    const pct = Number(cachePct)
    if (!Number.isFinite(pct) || pct < 0 || pct > 100) {
      toast.error('放大比例需在 0–100 之间')
      return
    }
    const ratio = pct / 100
    if (data?.perceivedCacheHitRatio !== null && ratio === data?.perceivedCacheHitRatio) return
    commit({ perceivedCacheHitRatio: ratio }, '缓存放大比例')
  }

  const isAffinity = mode === 'affinity'

  return (
    <div className="space-y-3 border-t pt-4">
      <div>
        <span className="text-sm font-medium">调度策略</span>
        <p className="text-xs text-muted-foreground mt-0.5">
          控制请求如何在多个上游账号间分配。改动即时生效并持久化。
        </p>
      </div>

      {isLoading ? (
        <div className="text-sm text-muted-foreground py-2 text-center">加载中...</div>
      ) : (
        <div className="space-y-3">
          {/* 模式 */}
          <div className="space-y-1">
            <label className="text-xs text-muted-foreground">负载均衡模式</label>
            <select
              value={mode}
              onChange={(e) => commitMode(e.target.value as SchedulingMode)}
              disabled={isPending}
              className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50"
            >
              {(['affinity', 'balanced', 'priority'] as SchedulingMode[]).map((m) => (
                <option key={m} value={m}>
                  {MODE_LABELS[m]}
                </option>
              ))}
            </select>
            <p className="text-xs text-muted-foreground">{MODE_HINTS[mode]}</p>
          </div>

          {/* 会话亲和参数（仅 affinity 模式可调） */}
          <div className={`grid grid-cols-2 gap-3 ${isAffinity ? '' : 'opacity-50'}`}>
            <div className="space-y-1">
              <label className="text-xs text-muted-foreground">次选转正阈值 K</label>
              <Input
                type="number"
                min={1}
                max={20}
                value={k}
                disabled={!isAffinity || isPending}
                onChange={(e) => setK(e.target.value)}
                onBlur={() => commitNumber(k, 'affinityPromoteThreshold', data?.affinityPromoteThreshold, '次选转正阈值')}
              />
              <p className="text-[11px] text-muted-foreground">连续命中次选 K 次后转正（1–20）</p>
            </div>
            <div className="space-y-1">
              <label className="text-xs text-muted-foreground">映射 TTL（秒）</label>
              <Input
                type="number"
                min={60}
                max={86400}
                value={ttl}
                disabled={!isAffinity || isPending}
                onChange={(e) => setTtl(e.target.value)}
                onBlur={() => commitNumber(ttl, 'affinityMapTtlSecs', data?.affinityMapTtlSecs, '映射 TTL')}
              />
              <p className="text-[11px] text-muted-foreground">会话多久不活动即淘汰（60–86400）</p>
            </div>
          </div>

          {/* 限流冷却（所有模式通用） */}
          <div className="space-y-1">
            <label className="text-xs text-muted-foreground">限流冷却（秒）</label>
            <Input
              type="number"
              min={5}
              max={600}
              value={cooldown}
              disabled={isPending}
              onChange={(e) => setCooldown(e.target.value)}
              onBlur={() => commitNumber(cooldown, 'cooldownSecs', data?.cooldownSecs, '限流冷却')}
            />
            <p className="text-[11px] text-muted-foreground">账号命中 429 后被跳过的冷却时长，到点自动恢复</p>
          </div>

          {/* 感知缓存放大（全局，作用于上报给中转网关的 usage） */}
          <div className="space-y-1 border-t pt-3">
            <label className="flex items-center gap-2 text-xs text-muted-foreground">
              <input
                type="checkbox"
                checked={cacheOn}
                disabled={isPending}
                onChange={(e) => commitCacheToggle(e.target.checked)}
                className="h-3.5 w-3.5"
              />
              感知缓存命中放大
            </label>
            <Input
              type="number"
              min={0}
              max={100}
              value={cachePct}
              disabled={!cacheOn || isPending}
              onChange={(e) => setCachePct(e.target.value)}
              onBlur={commitCachePct}
            />
            <p className="text-[11px] text-muted-foreground">
              命中时把上报给中转网关的 cache_read 比例直接覆盖为此百分比（0–100）。
              代理方承担与 Kiro 真实计费的差额。关闭则按真实/估算值上报。
            </p>
          </div>
        </div>
      )}
    </div>
  )
}
