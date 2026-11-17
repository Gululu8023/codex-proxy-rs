import accounts from './accounts.json'
import appearance from './appearance.json'
import auth from './auth.json'
import claude from './claude.json'
import common from './common.json'
import dashboard from './dashboard.json'
import groups from './groups.json'
import keys from './keys.json'
import navigation from './navigation.json'
import proxies from './proxies.json'
import settings from './settings.json'
import systemUpdate from './systemUpdate.json'
import usage from './usage.json'

export { zhHans as uiLocale } from '@codex-proxy/ui/locale'

export default {
  ...accounts,
  ...claude,
  ...appearance,
  ...auth,
  ...common,
  ...dashboard,
  ...groups,
  ...keys,
  ...navigation,
  ...proxies,
  ...settings,
  ...systemUpdate,
  ...usage,
}
