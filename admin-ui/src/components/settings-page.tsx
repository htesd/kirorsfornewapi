import { Network, Layers, KeyRound } from 'lucide-react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { SchedulingPanel } from '@/components/scheduling-panel'
import { GroupsPanel } from '@/components/groups-panel'
import { ApiKeysPanel } from '@/components/api-keys-panel'

/**
 * 设置页（顶部「设置」Tab 渲染）
 *
 * 取代原先塞进 512px 小弹窗的 SettingsDialog：
 * - 调度策略：模式 + 会话亲和参数 + 限流冷却 + 感知缓存放大
 * - 账号池分组：建组/改名/删组 + 账号归组
 * - 反代 API Key：增删/启禁 + 分组绑定
 *
 * 全宽布局，宽屏下调度策略与账号池分组并排，API Key 单独成行。
 */
export function SettingsPage() {
  return (
    <div className="space-y-6">
      <div className="grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <Network className="h-4 w-4" />
              调度策略
            </CardTitle>
          </CardHeader>
          <CardContent>
            <SchedulingPanel />
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <Layers className="h-4 w-4" />
              账号池分组
            </CardTitle>
          </CardHeader>
          <CardContent>
            <GroupsPanel />
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader className="pb-3">
          <CardTitle className="flex items-center gap-2 text-base">
            <KeyRound className="h-4 w-4" />
            反代 API Key
          </CardTitle>
          <p className="text-xs text-muted-foreground">
            客户端调用 /v1/messages 的访问密钥，可配置多个，任意一个均可通过认证。增删立即生效并持久化到 SQLite。
          </p>
        </CardHeader>
        <CardContent>
          <ApiKeysPanel />
        </CardContent>
      </Card>
    </div>
  )
}
