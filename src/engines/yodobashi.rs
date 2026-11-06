// SPDX-License-Identifier: AGPL-3.0-or-later
//! Yodobashi.com engine (no upstream searx equivalent).
// engine-id: yodobashi
//!
//! Yodobashi Camera's online store renders search results on the server: one
//! page holds 48 product tiles. The page number lives in the path (`/p2/`),
//! not in a query parameter — `/?word=…&p=2` returns page 1 — so the request
//! URL is assembled from both parts.
//!
//! `#listContents` also holds the Criteo promotions, wrapped one level deeper
//! (`div.srcResultItem > div.js_criteoFormat > div.srcResultItem_block`).
//! Selecting the organic tile as a *direct* child of `div.srcResultItem`
//! excludes them, so results do not depend on the sponsored class names.
//!
//! Every tile carries `<div class="pName"><p>maker</p><p>product name</p>`,
//! a `span.productPrice` and a `div.pSubInfo` with the release date. The price
//! and the maker are kept in the metadata as well as the snippet.
//!
//! HTML automation for this host has no recorded permission, so the shipped
//! settings entry is disabled (see docs/PROVIDERS.md).
//!
//! Measured on 2026-11-07 through this project's own network layer: the edge
//! answers 200 to a browser client and resets the stream for any other one.
//! With the shipped `[outgoing] identified_ua = true` the request never gets
//! past that check, so a search returns "Unexpected error" rather than
//! results. Nothing here works around it — the engine has no UA or header
//! override — and enabling the entry without a permission review would only
//! add a failing request to every search.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["shopping"],
    shortcut: "yod",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    // A search page is about 70 kB and the host answers slowly; 4 s is tight.
    timeout: 6.0,
};

const BASE_URL: &str = "https://www.yodobashi.com";

/// One organic product tile. Sponsored tiles are nested one level deeper, so
/// the child combinator keeps them out (see the module comment).
const RESULTS_SELECTOR: &str =
    "#listContents > div.srcResultItem > div.srcResultItem_block.pListBlock";

/// Yodobashi.com (ヨドバシカメラ).
pub struct Yodobashi {
    meta: EngineMeta,
}

impl Yodobashi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Yodobashi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        // Page 1 is "/", later pages are "/p2/", "/p3/", …; "/p1/" redirects.
        let path = if pageno <= 1 {
            "/".to_string()
        } else {
            format!("/p{pageno}/")
        };
        let pairs: Vec<(&str, String)> = vec![("word", query.query_text())];
        Ok(EngineRequest::get(format!(
            "{}{path}?{}",
            self.base_url(),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        // An Akamai denial is an HTML page, not an empty result set; report it
        // as such so the framework backs off instead of caching "no results".
        if response.status == 403
            || (body.contains("Access Denied") && body.contains("Reference #"))
        {
            return Err(EngineError::AccessDenied);
        }
        let html = parse_html(&body);
        let base = self.base_url();
        let mut results = Vec::new();
        for element in select_elements(&html, RESULTS_SELECTOR)? {
            let Some(href) = attr_of(element, "a[href^=\"/product/\"]", "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            let names: Vec<String> = children(element, "div.pName p")
                .into_iter()
                .map(text)
                .filter(|value| !value.is_empty())
                .collect();
            let (maker, title) = match names.len() {
                // The first paragraph is the maker, the last one the product.
                2.. => (names[0].clone(), names[names.len() - 1].clone()),
                // A product without a maker keeps the single paragraph as title.
                _ => (String::new(), names.last().cloned().unwrap_or_default()),
            };
            if title.is_empty() {
                continue;
            }
            let price = text_of(element, "span.productPrice").unwrap_or_default();
            let content = children(element, "div.pInfo li")
                .into_iter()
                .map(text)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" | ");
            let mut builder = ResultBuilder::new(url, title)
                .content(content)
                .metadata("price", price)
                .metadata("maker", maker)
                .metadata(
                    "release_date",
                    text_of(element, "div.pSubInfo span.gray").unwrap_or_default(),
                );
            if let Some(thumbnail) = attr_of(element, "div.pImg img", "src") {
                if !thumbnail.is_empty() {
                    builder = builder.thumbnail(thumbnail);
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from the live page: one Criteo promotion (nested one level
    /// deeper than the organic tile) and two organic products.
    const FIXTURE: &str = r#"
    <html><body><div class="mainColInner"><div id="listContents">
      <div class="srcResultItem spt_hznList tileTypeList">
        <div class="js_criteoFormat srcResultPromotionItem">
          <div class="srcResultItem_block pListBlock hznBox js_criteoProduct productListTile" data-sku="999">
            <a class="js_productListPostTag js-promotionLink" href="/product/999/">
              <div class="pImg mb15"><img src="https://image.yodobashi.com/promo.jpg" alt="promo"></div>
              <div class="pName fs14"><p>広告主</p><p>スポンサー商品</p></div>
            </a>
            <div class="pInfo"><ul><li><span class="productPrice">￥1</span></li></ul></div>
          </div>
        </div>
      </div>
      <div class="srcResultItem spt_hznList tileTypeList">
        <div class="srcResultItem_block pListBlock hznBox js_productBox js_criteoProduct productListTile" data-sku="100000001006659530">
          <div class="imgBody"><div class="imgOverlay"></div></div>
          <a class="js_productListPostTag js-promotionLink" href="/product/100000001006659530/">
            <div class="pImg mb15"><img alt="ワイヤレスマウス M221" src="https://image.yodobashi.com/product/100/000/001/006/659/530/100000001006659530_10205.jpg"></div>
            <div class="pName fs14"><p>ロジクール Logicool</p><p>ワイヤレスマウス M221 サイレント 無線（2.4GHz） 光学式 3ボタン M221CG</p></div>
          </a>
          <div class="pSubInfo pcParts"><span class="mr10"><a href="/?mkrbrnds=0000001826">ロジクール</a></span><span class="gray">2021/11/04</span></div>
          <div class="pInfo"><ul><li></li><li><span class="productPrice">￥1,480</span></li><li><span class="goldPoint orange">148<span class="unitPointSpace">&nbsp;</span><span class="orange unitPoint">ゴールドポイント</span></span></li><li><span class="green">在庫あり</span></li></ul></div>
        </div>
      </div>
      <div class="srcResultItem spt_hznList tileTypeList">
        <div class="srcResultItem_block pListBlock hznBox js_productBox productListTile" data-sku="100000001003173433">
          <a class="js_productListPostTag js-promotionLink" href="/product/100000001003173433/">
            <div class="pImg mb15"><img alt="ワイヤレスマウス ENELO" src="https://image.yodobashi.com/product/100/000/001/003/173/433/100000001003173433_10205_001.jpg"></div>
            <div class="pName fs14"><p>エレコム ELECOM</p><p>ワイヤレスマウス ENELOシリーズ 無線（2.4GHz） ブルー M-IR07DRBU</p></div>
          </a>
          <div class="pSubInfo pcParts"><span class="gray">2016/07/23</span></div>
          <div class="pInfo"><ul><li><span class="productPrice">￥944</span></li></ul></div>
        </div>
      </div>
    </div></div></body></html>"#;

    fn engine() -> Arc<Yodobashi> {
        Yodobashi::from_config(&EngineConfig::new(
            "yodobashi",
            "yodobashi",
            "yod",
            &["shopping"],
        ))
        .unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_organic_tiles_and_skips_promotions() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("マウス")).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.url,
                    "https://www.yodobashi.com/product/100000001006659530/"
                );
                assert_eq!(
                    result.title,
                    "ワイヤレスマウス M221 サイレント 無線（2.4GHz） 光学式 3ボタン M221CG"
                );
                assert_eq!(
                    result.metadata.get("price").map(String::as_str),
                    Some("￥1,480")
                );
                assert_eq!(
                    result.metadata.get("maker").map(String::as_str),
                    Some("ロジクール Logicool")
                );
                assert_eq!(
                    result.metadata.get("release_date").map(String::as_str),
                    Some("2021/11/04")
                );
                assert!(result.content.contains("在庫あり"));
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some(
                        "https://image.yodobashi.com/product/100/000/001/006/659/530/100000001006659530_10205.jpg"
                    )
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.metadata.get("maker").map(String::as_str),
                    Some("エレコム ELECOM")
                );
                assert_eq!(
                    result.metadata.get("release_date").map(String::as_str),
                    Some("2016/07/23")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn a_tile_without_a_maker_keeps_its_title() {
        let fixture = r#"<html><body><div id="listContents">
          <div class="srcResultItem"><div class="srcResultItem_block pListBlock">
            <a href="/product/1/"><div class="pName"><p>まとめ買い インクカートリッジ</p></div></a>
          </div></div>
        </div></body></html>"#;
        let response = EngineResponse::new(200, BASE_URL, fixture.as_bytes().to_vec());
        let items = engine().parse(&response, &query("インク")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "まとめ買い インクカートリッジ");
                assert!(!result.metadata.contains_key("maker"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn an_empty_result_page_yields_no_items() {
        let fixture =
            r#"<html><body><p>"zzz"の検索に一致する商品は見つかりませんでした</p></body></html>"#;
        let response = EngineResponse::new(200, BASE_URL, fixture.as_bytes().to_vec());
        assert!(engine().parse(&response, &query("zzz")).unwrap().is_empty());
    }

    #[test]
    fn an_akamai_denial_is_reported_as_access_denied() {
        let fixture = r#"<html><body><h1>Access Denied</h1><p>Reference #18.abc</p></body></html>"#;
        let response = EngineResponse::new(403, BASE_URL, fixture.as_bytes().to_vec());
        assert!(matches!(
            engine().parse(&response, &query("マウス")),
            Err(EngineError::AccessDenied)
        ));
    }

    #[test]
    fn builds_the_first_and_later_pages() {
        let engine = engine();
        let request = engine
            .build_request(&query("ワイヤレス マウス"), 1)
            .unwrap();
        assert_eq!(
            request.url,
            "https://www.yodobashi.com/?word=%E3%83%AF%E3%82%A4%E3%83%A4%E3%83%AC%E3%82%B9%20%E3%83%9E%E3%82%A6%E3%82%B9"
        );
        let request = engine.build_request(&query("マウス"), 2).unwrap();
        assert_eq!(
            request.url,
            "https://www.yodobashi.com/p2/?word=%E3%83%9E%E3%82%A6%E3%82%B9"
        );
        assert!(request.headers.is_empty());
    }
}
