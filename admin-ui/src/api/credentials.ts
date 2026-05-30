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
}

export type UpdateSchedulingPayload = Partial<Scheduling>

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
