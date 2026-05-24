// 凭据状态响应
export interface CredentialsStatusResponse {
  total: number
  available: number
  currentId: number
  credentials: CredentialStatusItem[]
}

// 单个凭据状态
export interface CredentialStatusItem {
  id: number
  priority: number
  disabled: boolean
  failureCount: number
  isCurrent: boolean
  expiresAt: string | null
  authMethod: string | null
  hasProfileArn: boolean
  email?: string
  refreshTokenHash?: string
  apiKeyHash?: string
  maskedApiKey?: string
  successCount: number
  lastUsedAt: string | null
  hasProxy: boolean
  proxyUrl?: string
  refreshFailureCount: number
  disabledReason?: string
  endpoint: string
  maxConcurrency: number
  inFlight: number
}

// 请求日志列表项
export interface RequestLogSummary {
  requestId: string
  tsMs: number
  model: string
  upstreamModel?: string
  endpoint: string
  accountId?: string
  accountLabel?: string
  status: string
  httpStatus?: number
  errorKind?: string
  reason?: string
  attempts: number
  isStream: boolean
  latencyMs: number
  ttfbMs?: number
  promptTokens?: number
  completionTokens?: number
  cachedTokens?: number
  meteringUnit?: string
  meteringUsage?: number
  contextUsagePct?: number
}

export interface RequestLogDetail extends RequestLogSummary {
  messagesCount?: number
  toolsCount?: number
  systemPromptLen?: number
  hasCacheControl?: boolean
  errorMessage?: string
  errorStage?: string
  requestBody?: string
  responseBody?: string
}

export interface RequestLogListResponse {
  total: number
  items: RequestLogSummary[]
}

// 余额响应
export interface BalanceResponse {
  id: number
  subscriptionTitle: string | null
  currentUsage: number
  usageLimit: number
  remaining: number
  usagePercentage: number
  nextResetAt: number | null
}

// 成功响应
export interface SuccessResponse {
  success: boolean
  message: string
}

// 错误响应
export interface AdminErrorResponse {
  error: {
    type: string
    message: string
  }
}

// 请求类型
export interface SetDisabledRequest {
  disabled: boolean
}

export interface SetPriorityRequest {
  priority: number
}

// 添加凭据请求
export interface AddCredentialRequest {
  refreshToken?: string
  authMethod?: 'social' | 'idc' | 'api_key'
  clientId?: string
  clientSecret?: string
  priority?: number
  authRegion?: string
  apiRegion?: string
  machineId?: string
  proxyUrl?: string
  proxyUsername?: string
  proxyPassword?: string
  kiroApiKey?: string
  endpoint?: string
}

// 添加凭据响应
export interface AddCredentialResponse {
  success: boolean
  message: string
  credentialId: number
  email?: string
}
