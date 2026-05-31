import axios from 'axios'
import { storage } from '@/lib/storage'
import type {
  CredentialsStatusResponse,
  BalanceResponse,
  SuccessResponse,
  SetDisabledRequest,
  SetPriorityRequest,
  AddCredentialRequest,
  AddCredentialResponse,
  RequestLogListResponse,
  RequestLogDetail,
} from '@/types/api'

// 创建 axios 实例
const api = axios.create({
  baseURL: '/api/admin',
  headers: {
    'Content-Type': 'application/json',
  },
})

// 请求拦截器添加 API Key
api.interceptors.request.use((config) => {
  const apiKey = storage.getApiKey()
  if (apiKey) {
    config.headers['x-api-key'] = apiKey
  }
  return config
})

// 获取所有凭据状态
export async function getCredentials(): Promise<CredentialsStatusResponse> {
  const { data } = await api.get<CredentialsStatusResponse>('/credentials')
  return data
}

// 设置凭据禁用状态
export async function setCredentialDisabled(
  id: number,
  disabled: boolean
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/disabled`,
    { disabled } as SetDisabledRequest
  )
  return data
}

// 设置凭据优先级
export async function setCredentialPriority(
  id: number,
  priority: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/priority`,
    { priority } as SetPriorityRequest
  )
  return data
}

// 重置失败计数
export async function resetCredentialFailure(
  id: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/credentials/${id}/reset`)
  return data
}

// 强制刷新 Token
export async function forceRefreshToken(
  id: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/credentials/${id}/refresh`)
  return data
}

// 获取凭据余额
export async function getCredentialBalance(id: number): Promise<BalanceResponse> {
  const { data } = await api.get<BalanceResponse>(`/credentials/${id}/balance`)
  return data
}

// 添加新凭据
export async function addCredential(
  req: AddCredentialRequest
): Promise<AddCredentialResponse> {
  const { data } = await api.post<AddCredentialResponse>('/credentials', req)
  return data
}

// 删除凭据
export async function deleteCredential(id: number): Promise<SuccessResponse> {
  const { data } = await api.delete<SuccessResponse>(`/credentials/${id}`)
  return data
}

// 获取负载均衡模式
export async function getLoadBalancingMode(): Promise<{ mode: 'priority' | 'balanced' }> {
  const { data } = await api.get<{ mode: 'priority' | 'balanced' }>('/config/load-balancing')
  return data
}

// 设置负载均衡模式
export async function setLoadBalancingMode(mode: 'priority' | 'balanced'): Promise<{ mode: 'priority' | 'balanced' }> {
  const { data } = await api.put<{ mode: 'priority' | 'balanced' }>('/config/load-balancing', { mode })
  return data
}

// 获取限流冷却时长（秒）
export async function getRateLimitCooldown(): Promise<{ cooldownSecs: number }> {
  const { data } = await api.get<{ cooldownSecs: number }>('/config/rate-limit-cooldown')
  return data
}

// 设置限流冷却时长（秒）
export async function setRateLimitCooldown(cooldownSecs: number): Promise<{ cooldownSecs: number }> {
  const { data } = await api.put<{ cooldownSecs: number }>('/config/rate-limit-cooldown', { cooldownSecs })
  return data
}

// 调度策略（模式 + 冷却 + 会话亲和参数）
export type SchedulingMode = 'priority' | 'balanced' | 'affinity'

export interface Scheduling {
  mode: SchedulingMode
  cooldownSecs: number
  affinityPromoteThreshold: number
  affinityMapTtlSecs: number
  /** 感知缓存命中放大比例（0-1；null = 未启用放大，按真实值上报） */
  perceivedCacheHitRatio: number | null
}

export type UpdateSchedulingPayload = Partial<{
  mode: SchedulingMode
  cooldownSecs: number
  affinityPromoteThreshold: number
  affinityMapTtlSecs: number
  /** 设置放大比例（0-1）。与 disablePerceivedCache 互斥 */
  perceivedCacheHitRatio: number
  /** true = 关闭放大（设为 null） */
  disablePerceivedCache: boolean
}>

// 获取调度策略全部参数
export async function getScheduling(): Promise<Scheduling> {
  const { data } = await api.get<Scheduling>('/config/scheduling')
  return data
}

// 更新调度策略（仅传需要改的字段）
export async function updateScheduling(payload: UpdateSchedulingPayload): Promise<Scheduling> {
  const { data } = await api.put<Scheduling>('/config/scheduling', payload)
  return data
}

// 反代访问密钥（脱敏）
export interface ApiKeyItem {
  id: number
  masked: string
  label: string | null
  createdAt: number
  disabled: boolean
  /** 绑定的分组 id（null = 未绑定，可用全部账号） */
  groupId: number | null
}

// 列出全部反代访问密钥
export async function listApiKeys(): Promise<{ keys: ApiKeyItem[] }> {
  const { data } = await api.get<{ keys: ApiKeyItem[] }>('/api-keys')
  return data
}

// 新增反代访问密钥
export async function addApiKey(key: string, label?: string): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>('/api-keys', { key, label })
  return data
}

// 删除反代访问密钥
export async function deleteApiKey(id: number): Promise<SuccessResponse> {
  const { data } = await api.delete<SuccessResponse>(`/api-keys/${id}`)
  return data
}

// 设置反代访问密钥的启用/禁用状态
export async function setApiKeyDisabled(id: number, disabled: boolean): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/api-keys/${id}/disabled`, { disabled })
  return data
}

// 设置凭据并发上限
export async function setCredentialConcurrency(
  id: number,
  maxConcurrency: number,
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/concurrency`,
    { maxConcurrency },
  )
  return data
}

// 请求日志列表
export async function listRequests(params: {
  limit?: number
  offset?: number
  status?: string
  accountId?: string
} = {}): Promise<RequestLogListResponse> {
  const { data } = await api.get<RequestLogListResponse>('/requests', { params })
  return data
}

// 请求日志详情
export async function getRequestDetail(id: string): Promise<RequestLogDetail> {
  const { data } = await api.get<RequestLogDetail>(`/requests/${encodeURIComponent(id)}`)
  return data
}

// ============ 账号池分组 ============

export interface GroupItem {
  id: number
  name: string
  createdAt: number
  /** 该分组下的凭据 id 列表 */
  credentialIds: number[]
}

// 列出全部分组（含成员凭据 id）
export async function listGroups(): Promise<{ groups: GroupItem[] }> {
  const { data } = await api.get<{ groups: GroupItem[] }>('/groups')
  return data
}

// 新建分组
export async function addGroup(name: string): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>('/groups', { name })
  return data
}

// 重命名分组
export async function renameGroup(id: number, name: string): Promise<SuccessResponse> {
  const { data } = await api.put<SuccessResponse>(`/groups/${id}`, { name })
  return data
}

// 删除分组（级联清空归属、解绑 apikey）
export async function deleteGroup(id: number): Promise<SuccessResponse> {
  const { data } = await api.delete<SuccessResponse>(`/groups/${id}`)
  return data
}

// 设置某凭据的分组归属（groupId=null 移出分组）
export async function setCredentialGroup(
  id: number,
  groupId: number | null,
): Promise<SuccessResponse> {
  const { data } = await api.put<SuccessResponse>(`/credentials/${id}/group`, { groupId })
  return data
}

// 设置某 apikey 的分组绑定（groupId=null 解绑）
export async function setApiKeyGroup(
  id: number,
  groupId: number | null,
): Promise<SuccessResponse> {
  const { data } = await api.put<SuccessResponse>(`/api-keys/${id}/group`, { groupId })
  return data
}
