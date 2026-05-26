use std::borrow::Cow;
use std::collections::HashMap;

mod en;
mod zh_cn;
mod zh_tw;

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum Lang {
    En,
    ZhCn,
    ZhTw,
}

pub struct I18n {
    pub lang: Lang,
    strings: HashMap<&'static str, HashMap<Lang, &'static str>>,
}

impl I18n {
    pub fn new(lang: Lang) -> Self {
        let mut strings = HashMap::new();
        Self::load_all(&mut strings);
        I18n { lang, strings }
    }

    pub fn switch(&mut self, lang: Lang) {
        self.lang = lang;
    }

    pub fn t(&self, key: &str) -> Cow<'_, str> {
        self.strings
            .get(key)
            .and_then(|m| m.get(&self.lang))
            .copied()
            .map_or_else(|| Cow::Owned(key.to_string()), Cow::Borrowed)
    }

    fn load_all(m: &mut HashMap<&'static str, HashMap<Lang, &'static str>>) {
        for (key, val) in en::all() {
            m.entry(key)
                .or_insert_with(HashMap::new)
                .insert(Lang::En, val);
        }
        for (key, val) in zh_cn::all() {
            m.entry(key)
                .or_insert_with(HashMap::new)
                .insert(Lang::ZhCn, val);
        }
        for (key, val) in zh_tw::all() {
            m.entry(key)
                .or_insert_with(HashMap::new)
                .insert(Lang::ZhTw, val);
        }
    }
}

// Kept for backward compatibility. The string data was migrated to
// static arrays in en.rs / zh_cn.rs / zh_tw.rs, but external consumers
// of the crate may still use `tr!()` at call sites.
#[allow(unused_macros)]
macro_rules! tr {
    ($_en:expr, $en_val:expr, $_cn:expr, $cn_val:expr, $_tw:expr, $tw_val:expr) => {{
        let mut m = std::collections::HashMap::new();
        m.insert($crate::i18n::Lang::En, $en_val);
        m.insert($crate::i18n::Lang::ZhCn, $cn_val);
        m.insert($crate::i18n::Lang::ZhTw, $tw_val);
        m
    }};
}
#[allow(unused_imports)]
pub(crate) use tr;
