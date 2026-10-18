use ru_marketplace_core::domain::{Currency, Marketplace, PriceTier};
use ru_marketplace_dns::DnsConnector;

#[test]
fn test_dns_parse_catalog_data() {
    let mock_json = r#"[
        {
            "name": "6.1\" Смартфон Apple iPhone 15 128 ГБ черный",
            "url": "https://www.dns-shop.ru/product/12345/iphone-15/",
            "price": 74999,
            "sku": "54321"
        }
    ]"#;

    let offers = DnsConnector::parse_catalog_data(mock_json, 5).unwrap();
    assert_eq!(offers.len(), 1);

    let offer = &offers[0];
    assert_eq!(offer.marketplace, Marketplace::Dns);
    assert_eq!(offer.title, "6.1\" Смартфон Apple iPhone 15 128 ГБ черный");
    assert_eq!(offer.price_tier, PriceTier::Base(74999));
    assert_eq!(offer.currency, Currency::Rub);
    assert_eq!(offer.seller, Some("DNS".to_string()));
}

#[test]
fn test_dns_parse_modern_html_catalog() {
    let html = r#"
    <!DOCTYPE html>
    <html>
    <body>
        <div class="products-list">
            <div class="catalog-product ui-button-widget" data-product="1029384" data-code="1029384">
                <a class="catalog-product__name ui-link ui-link_black" href="/product/1029384/processor-intel-core-i5-14600k/">
                    <span>Процессор Intel Core i5-14600K OEM</span>
                </a>
                <div class="catalog-product__price">
                    <div class="product-buy__price product-buy__price_active">27 999&nbsp;₽</div>
                </div>
                <div class="catalog-product__rating" data-rating="4.8">4.8 (64)</div>
            </div>
            <div class="catalog-product ui-button-widget" data-product="1029385" data-code="1029385">
                <a class="catalog-product__name ui-link ui-link_black" href="/product/1029385/processor-amd-ryzen-7-7800x3d/">
                    <span>Процессор AMD Ryzen 7 7800X3D OEM</span>
                </a>
                <div class="catalog-product__price">
                    <div class="product-buy__price product-buy__price_active">41 499 ₽</div>
                </div>
            </div>
        </div>
    </body>
    </html>
    "#;

    let offers = DnsConnector::parse_catalog_data(html, 10).unwrap();
    assert_eq!(offers.len(), 2);

    let first = &offers[0];
    assert_eq!(first.marketplace, Marketplace::Dns);
    assert_eq!(first.title, "Процессор Intel Core i5-14600K OEM");
    assert_eq!(first.price_tier, PriceTier::Base(27999));
    assert_eq!(
        first.url,
        "https://www.dns-shop.ru/product/1029384/processor-intel-core-i5-14600k/"
    );
    assert_eq!(first.product_id, Some("1029384".to_string()));
    assert_eq!(first.seller, Some("DNS".to_string()));
    assert_eq!(first.rating, Some(4.8));

    let second = &offers[1];
    assert_eq!(second.title, "Процессор AMD Ryzen 7 7800X3D OEM");
    assert_eq!(second.price_tier, PriceTier::Base(41499));
    assert_eq!(second.product_id, Some("1029385".to_string()));
}
