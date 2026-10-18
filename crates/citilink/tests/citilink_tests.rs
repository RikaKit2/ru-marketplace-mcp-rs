use ru_marketplace_citilink::CitilinkConnector;
use ru_marketplace_core::domain::{Currency, Marketplace, PriceTier};

#[test]
fn test_citilink_parse_catalog_data() {
    let mock_json = r#"[
        {
            "name": "Смартфон Apple iPhone 15 128Gb, черный",
            "url": "https://www.citilink.ru/product/smartfon-apple-iphone-15-128gb-1987654/",
            "offers": {
                "price": 72990
            },
            "sku": "1987654"
        }
    ]"#;

    let offers = CitilinkConnector::parse_catalog_data(mock_json, 5).unwrap();
    assert_eq!(offers.len(), 1);

    let offer = &offers[0];
    assert_eq!(offer.marketplace, Marketplace::Citilink);
    assert_eq!(offer.title, "Смартфон Apple iPhone 15 128Gb, черный");
    assert_eq!(offer.price_tier, PriceTier::Base(72990));
    assert_eq!(offer.currency, Currency::Rub);
    assert_eq!(offer.seller, Some("Citilink".to_string()));
}

#[test]
fn test_citilink_parse_modern_html_catalog() {
    let html = r#"
    <!DOCTYPE html>
    <html>
    <body>
        <div class="product-cards-layout">
            <div class="app-catalog-1tp040a" data-meta-name="ProductCard" data-meta-product-id="1948271">
                <a class="app-catalog-9ff07x" data-meta-name="ProductCard__title" title="Процессор Intel Core i5 14600K LGA 1700 OEM" href="/product/processor-intel-core-i5-14600k-lga-1700-oem-1948271/">
                    Процессор Intel Core i5 14600K LGA 1700 OEM
                </a>
                <div data-meta-name="ProductCard__price">
                    <span class="app-catalog-price" data-meta-price="28700">28 700 ₽</span>
                </div>
                <div data-meta-name="ProductCard__rating" data-meta-rating="4.9">4.9</div>
                <div data-meta-name="ProductCard__reviews">28 отзывов</div>
            </div>
            <div class="app-catalog-1tp040a" data-meta-name="SnippetProduct" data-meta-product-id="1948272">
                <a data-meta-name="SnippetProduct__title" title="Процессор AMD Ryzen 7 7800X3D AM5 OEM" href="/product/processor-amd-ryzen-7-7800x3d-am5-oem-1948272/">
                    Процессор AMD Ryzen 7 7800X3D AM5 OEM
                </a>
                <div data-meta-name="SnippetProduct__price">
                    <span>42 990&nbsp;₽</span>
                </div>
            </div>
        </div>
    </body>
    </html>
    "#;

    let offers = CitilinkConnector::parse_catalog_data(html, 10).unwrap();
    assert_eq!(offers.len(), 2);

    let first = &offers[0];
    assert_eq!(first.marketplace, Marketplace::Citilink);
    assert_eq!(first.title, "Процессор Intel Core i5 14600K LGA 1700 OEM");
    assert_eq!(first.price_tier, PriceTier::Base(28700));
    assert_eq!(
        first.url,
        "https://www.citilink.ru/product/processor-intel-core-i5-14600k-lga-1700-oem-1948271/"
    );
    assert_eq!(first.product_id, Some("1948271".to_string()));
    assert_eq!(first.seller, Some("Citilink".to_string()));
    assert_eq!(first.rating, Some(4.9));
    assert_eq!(first.review_count, Some(28));

    let second = &offers[1];
    assert_eq!(second.title, "Процессор AMD Ryzen 7 7800X3D AM5 OEM");
    assert_eq!(second.price_tier, PriceTier::Base(42990));
    assert_eq!(second.product_id, Some("1948272".to_string()));
}

#[test]
fn test_citilink_parse_next_data_script() {
    let html = r#"
    <!DOCTYPE html>
    <html>
    <body>
        <script id="__NEXT_DATA__" type="application/json">
        {
            "props": {
                "pageProps": {
                    "initialState": {
                        "products": [
                            {
                                "id": "1892831",
                                "name": "Кулер для процессора Deepcool AK620",
                                "price": 5490,
                                "url": "/product/kuler-deepcool-ak620-1892831/"
                            }
                        ]
                    }
                }
            }
        }
        </script>
    </body>
    </html>
    "#;

    let offers = CitilinkConnector::parse_catalog_data(html, 10).unwrap();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].title, "Кулер для процессора Deepcool AK620");
    assert_eq!(offers[0].price_tier, PriceTier::Base(5490));
    assert_eq!(offers[0].product_id, Some("1892831".to_string()));
}

#[test]
fn test_citilink_parse_vertical_snippet_html() {
    let html = r#"
    <!DOCTYPE html>
    <html>
    <body>
        <div data-meta-name="ProductVerticalSnippet">
            <div data-meta-name="Snippet__title">
                <a href="/product/noutbuk-chuwi-corebook-x-ryzen-5-5500u-16gb-ssd512gb-vega-7-14-2k-w11h-2174077/">
                    Ноутбук CHUWI Corebook X, 14", IPS, AMD Ryzen 5 5500U, серый
                </a>
            </div>
            <div data-meta-name="Snippet__price">47 990₽</div>
            <div data-meta-name="MetaInfo_rating">4.8</div>
            <div data-meta-name="MetaInfo_opinionsCount">12 отзывов</div>
        </div>
    </body>
    </html>
    "#;

    let offers = CitilinkConnector::parse_catalog_data(html, 10).unwrap();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].marketplace, Marketplace::Citilink);
    assert_eq!(
        offers[0].title,
        "Ноутбук CHUWI Corebook X, 14\", IPS, AMD Ryzen 5 5500U, серый"
    );
    assert_eq!(offers[0].price_tier, PriceTier::Base(47990));
    assert_eq!(offers[0].product_id, Some("2174077".to_string()));
    assert_eq!(offers[0].rating, Some(4.8));
    assert_eq!(offers[0].review_count, Some(12));
}

#[test]
fn test_citilink_parse_next_data_subcategory_payload() {
    let html = r#"
    <!DOCTYPE html>
    <html>
    <body>
        <script id="__NEXT_DATA__" type="application/json">
        {
            "props": {
                "pageProps": {
                    "initialState": {
                        "subcategory": {
                            "productList": {
                                "payload": {
                                    "products": [
                                        {
                                            "id": "2174077",
                                            "name": "Ноутбук CHUWI Corebook X",
                                            "price": 47990,
                                            "url": "/product/noutbuk-chuwi-corebook-x-2174077/"
                                        }
                                    ]
                                }
                            }
                        }
                    }
                }
            }
        }
        </script>
    </body>
    </html>
    "#;

    let offers = CitilinkConnector::parse_catalog_data(html, 10).unwrap();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].title, "Ноутбук CHUWI Corebook X");
    assert_eq!(offers[0].price_tier, PriceTier::Base(47990));
    assert_eq!(offers[0].product_id, Some("2174077".to_string()));
}
