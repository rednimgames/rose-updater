slint::slint! {
    import { Button, ListView } from "std-widgets.slint";

    import "res/font/Poppins-Bold.ttf";
    import "res/font/Poppins-Light.ttf";
    import "res/font/Poppins-Medium.ttf";
    import "res/font/Poppins-Regular.ttf";

    export component WindowButton {
        in property <image> icon;
        in property <brush> hover-color: #1817178b;

        callback clicked;

        height: 35px;
        width: 35px;

        btn-background := Rectangle {
            width: 100%;
            height: 100%;

            background: btn-ta.has-hover ? root.hover-color : transparent;
            drop-shadow-color: transparent;
            drop-shadow-offset-y: 1px;
        }

        btn-ta := TouchArea {
            height: 100%;
            width: 100%;
            mouse-cursor: pointer;

            Image {
                source: root.icon;
            }

            clicked => {
                root.clicked();
            }
        }
    }

    export component ProgressBar {
        min-height: 56px;
        min-width: 546px;

        ta := TouchArea {
            width: 100%;
            height: 100%;
        }

        background := Rectangle {
            width: 100%;
            height: 100%;
            background: #000000;
            border-radius: 4px;
        }

        Image {
            x: 0;
            y: 0;
            image-fit: fill;
            width: parent.width;
            height: parent.height;
            source: @image-url("res/progressbar-loading.png");
        }

        Text {
            font-family: "Poppins";
            font-size: 16px;
            text: "Progress";
        }
    }

    export component LaunchButton {
        callback clicked;

        height: 56px;
        width: 196px;

        btn-ta := TouchArea {
            width: parent.width;
            height: parent.height;
            mouse-cursor: pointer;

            clicked => {
                root.clicked();
            }
        }

        btn-image := Image {
            width: parent.width;
            height: parent.height;
            source: @image-url("res/button-update.png");
        }
    }

    export component MainWindow inherits Window {
        callback move_window(length, length);
        callback minimize();
        callback close();

        title: "ROSE Online Updater";
        width: 800px;
        height: 600px;
        icon: @image-url("res/client.png");
        default-font-family: "Poppins";
        default-font-size: 14px;
        no-frame: true;

        Image {
            source: @image-url("res/bg.png");
            width: 800px;
            height: 600px;
        }

        VerticalLayout {
            HorizontalLayout {
                TouchArea {
                    height: 35px;

                    moved => {
                        root.move_window(self.mouse-x - self.pressed-x, self.mouse-y - self.pressed-y);
                    }
                }

                WindowButton {
                    icon: @image-url("res/icon-settings.svg");
                }

                WindowButton {
                    icon: @image-url("res/icon-minimize.svg");

                    clicked => {
                        root.minimize();
                    }
                }

                WindowButton {
                    icon: @image-url("res/icon-close.svg");
                    hover-color: #5c0404;

                    clicked => {
                        root.close();
                    }
                }

            }
            HorizontalLayout {
                alignment: center;
                Image {
                    source: @image-url("res/roseonline.png");
                    height: 100px;
                    width: 177px;
                }
            }

            ListView {

            }

            HorizontalLayout {
                spacing: 10px;
                padding-left: 10px;
                padding-bottom: 5px;
                padding-right: 10px;

                ProgressBar {}
                LaunchButton {}
            }

            HorizontalLayout {
                padding-left: 10px;
                padding-bottom: 5px;
                Image {
                    height: 20px;
                    width: 80px;
                    source: @image-url("res/rednimgames.png");
                }
            }
        }

    }
}

use i_slint_backend_winit::WinitWindowAccessor;

fn main() -> anyhow::Result<()> {
    // Force winit backend so we can access winit window to do manual minimize
    // See: https://github.com/slint-ui/slint/issues/4400
    let winit_backend = i_slint_backend_winit::Backend::new()?;
    slint::platform::set_platform(Box::new(winit_backend)).unwrap();

    let main_window = MainWindow::new()?;

    let main_window_ref = main_window.as_weak();
    main_window.on_move_window(move |delta_x, delta_y| {
        let main_window_ref = main_window_ref.unwrap();
        let pos = main_window_ref
            .window()
            .position()
            .to_logical(main_window_ref.window().scale_factor());

        main_window_ref
            .window()
            .set_position(slint::LogicalPosition::new(
                pos.x + delta_x,
                pos.y + delta_y,
            ));
    });

    let main_window_ref = main_window.as_weak();
    main_window.on_minimize(move || {
        let main_window_ref = main_window_ref.unwrap();
        main_window_ref
            .window()
            .with_winit_window(|winit_window: &winit::window::Window| {
                winit_window.set_minimized(true);
            });
    });

    let main_window_ref = main_window.as_weak();
    main_window.on_close(move || {
        let main_window_ref = main_window_ref.unwrap();
        main_window_ref.window().hide().unwrap();
    });

    main_window.run()?;

    Ok(())
}
